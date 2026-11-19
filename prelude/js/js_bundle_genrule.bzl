# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

load("@prelude//:cache_mode.bzl", "CacheModeInfo")
load("@prelude//:genrule_local_labels.bzl", "resolved_genrule_labels_require_local")
load("@prelude//:genrule_prefer_local_labels.bzl", "resolved_genrule_labels_prefer_local")
load("@prelude//:is_full_meta_repo.bzl", "is_full_meta_repo")
load("@prelude//:prelude_rule.bzl", "prelude_rule")
load("@prelude//android:android_providers.bzl", "AndroidResourceInfo", "merge_android_packageable_info")
load("@prelude//js:js_providers.bzl", "JsBundleInfo")
load("@prelude//js:js_utils.bzl", "TRANSFORM_PROFILES", "get_apple_resource_providers_for_js_bundle", "get_bundle_name")
load("@prelude//os_lookup:defs.bzl", "Os", "OsLookup")
load("@prelude//utils:expect.bzl", "expect")
load("@prelude//utils:utils.bzl", "value_or")
load(":attrs.bzl", "js_bundle_genrule_attrs")

_GENRULE_OUT_DIR = "out"

_WINDOWS_ENV_SUBSTITUTIONS = [
    # Replace $OUT and ${OUT}
    (regex("\\$(OUT\\b|\\{OUT\\})"), "%OUT%"),
    (regex("\\$(SRCDIR\\b|\\{SRCDIR\\})"), "%SRCDIR%"),
    (regex("\\$(SRCS\\b|\\{SRCS\\})"), "%SRCS%"),
    (regex("\\$(TMP\\b|\\{TMP\\})"), "%TMP%"),
]

# We don't want to use cache mode in open source because the config keys that drive it aren't wired up
_USE_CACHE_MODE = is_full_meta_repo()

def _get_cache_mode(ctx: AnalysisContext) -> CacheModeInfo:
    if _USE_CACHE_MODE:
        return ctx.attrs._cache_mode[CacheModeInfo]
    else:
        return CacheModeInfo(allow_cache_uploads = False, cache_bust_genrules = False)

def _run_genrule(ctx: AnalysisContext, out_name: str, extra_env_vars: dict, identifier: str) -> Artifact:
    local_only = resolved_genrule_labels_require_local(ctx.attrs.labels)
    prefer_local = resolved_genrule_labels_prefer_local(ctx.attrs.labels)

    # NOTE: Eventually we shouldn't require local_only here, since we should be
    # fine with caching local fallbacks if necessary (or maybe that should be
    # disallowed as a matter of policy), but for now let's be safe.
    cacheable = value_or(ctx.attrs.cacheable, True) and (local_only or prefer_local)

    content_based = ctx.attrs.has_content_based_path

    out_artifact = ctx.actions.declare_output(_GENRULE_OUT_DIR, out_name, has_content_based_path = content_based)

    is_windows = ctx.attrs._exec_os_type[OsLookup].os == Os("windows")
    if is_windows:
        path_sep = "\\"
        cmd = ctx.attrs.cmd_exe if ctx.attrs.cmd_exe != None else ctx.attrs.cmd
        if cmd == None:
            fail("One of `cmd` or `cmd_exe` should be set.")
    else:
        path_sep = "/"
        cmd = ctx.attrs.bash if ctx.attrs.bash != None else ctx.attrs.cmd
        if cmd == None:
            fail("One of `cmd` or `bash` should be set.")

    replace_regex = []

    # For backwards compatibility with Buck1.
    if is_windows:
        for re, sub in _WINDOWS_ENV_SUBSTITUTIONS:
            replace_regex.append((re, sub))

        for extra_env_var in extra_env_vars:
            replace_regex.append(
                (regex("\\$(%s\\b|\\{%s\\})" % (extra_env_var, extra_env_var)), "%%%s%%" % extra_env_var),
            )

    cmd = cmd_args(cmd, replace_regex = replace_regex)

    if type(ctx.attrs.srcs) == type([]):
        # FIXME: We should always use the short_path, but currently that is sometimes blank.
        symlinks = {src.short_path: src for src in ctx.attrs.srcs}

        if len(symlinks) != len(ctx.attrs.srcs):
            for src in ctx.attrs.srcs:
                name = src.short_path
                if symlinks[name] != src:
                    msg = "genrule srcs include duplicative name: `{}`. ".format(name)
                    msg += "`{}` conflicts with `{}`".format(symlinks[name].owner, src.owner)
                    fail(msg)
    else:
        symlinks = ctx.attrs.srcs
    srcs_artifact = ctx.actions.symlinked_dir(
        "{}-srcs".format(identifier),
        symlinks,
        has_content_based_path = content_based,
    )

    delimiter = ctx.attrs.environment_expansion_separator or " "

    # Setup environment variables.
    srcs = cmd_args(delimiter = delimiter)
    for symlink in symlinks:
        srcs.add(cmd_args(srcs_artifact, format = path_sep.join([".", "{}", symlink.replace("/", path_sep)])))
    env_vars = {
        "OUT": out_artifact.as_output(),
        "SRCDIR": cmd_args(srcs_artifact, format = path_sep.join([".", "{}"])),
        "SRCS": srcs,
    } | {k: cmd_args(v) for k, v in getattr(ctx.attrs, "env", {}).items()}

    # RE will cache successful actions that don't produce the desired outputs,
    # so if that happens and _then_ we add a local-only label, we'll get a
    # cache hit on the action that didn't produce the outputs and get the error
    # again (thus making the label useless). So, when a local-only label is
    # set, we make the action *different*.
    if local_only:
        env_vars["__BUCK2_LOCAL_ONLY_CACHE_BUSTER"] = ""

    # see comment above
    if prefer_local:
        env_vars["__BUCK2_PREFER_LOCAL_CACHE_BUSTER"] = ""

    # For now, when uploads are enabled, be safe and avoid sharing cache hits.
    if cacheable and _get_cache_mode(ctx).cache_bust_genrules:
        env_vars["__BUCK2_ALLOW_CACHE_UPLOADS_CACHE_BUSTER"] = ""

    for key, value in extra_env_vars.items():
        env_vars[key] = value

    if is_windows:
        script_extension = "bat"
        script = [
            cmd_args('if NOT "%TEMP%" == "" set "TMP=%TEMP%"'),
            cmd,
        ]
    else:
        script_extension = "sh"
        script = [
            cmd_args("export TMP=${TMPDIR:-/tmp}"),
            cmd,
        ]

    # `cd` into the sandboxed source dir and relativize all paths to that, unless
    # the genrule explicitly opts in to running from the build root.
    if not ctx.attrs.repo_relative_root:
        srcs_dir = srcs_artifact

        if is_windows:
            rewrite_scratch_path = cmd_args(
                cmd_args(ctx.label.project_root, relative_to = srcs_artifact),
                format = 'set "BUCK_SCRATCH_PATH={}\\%BUCK_SCRATCH_PATH%"',
            )
        else:
            srcs_dir = cmd_args(srcs_dir, quote = "shell")
            rewrite_scratch_path = cmd_args(
                cmd_args(ctx.label.project_root, quote = "shell", relative_to = srcs_artifact),
                format = "export BUCK_SCRATCH_PATH={}/$BUCK_SCRATCH_PATH",
            )

        for script_cmd in script:
            script_cmd.relative_to(srcs_artifact)

        script = [
            rewrite_scratch_path,
            cmd_args(srcs_dir, format = "cd {}"),
        ] + script

        env_vars = {key: cmd_args(value, relative_to = srcs_artifact) for key, value in env_vars.items()}

    if is_windows:
        # Odd, why is this a single string. How does it not end up getting quoted and being weird?
        script = [cmd_args("@echo off")] + script

    sh_script, _ = ctx.actions.write(
        "sh/{}-genrule.{}".format(identifier, script_extension),
        script,
        is_executable = True,
        allow_args = True,
        has_content_based_path = content_based,
    )
    if is_windows:
        script_args = ["cmd.exe", "/v:off", "/c", sh_script]
    else:
        script_args = ["/usr/bin/env", "bash", "-e", sh_script]

    # Only set metadata arguments when they are non-null
    metadata_args = {}
    if ctx.attrs.metadata_env_var:
        metadata_args["metadata_env_var"] = ctx.attrs.metadata_env_var
    if ctx.attrs.metadata_path:
        metadata_args["metadata_path"] = ctx.attrs.metadata_path
    if ctx.attrs.remote_execution_dependencies:
        metadata_args["remote_execution_dependencies"] = ctx.attrs.remote_execution_dependencies

    # As of 09/2021, all genrule types were legal snake case if their dashes and periods were replaced with underscores.
    category = "genrule_" + ctx.attrs.type.replace("-", "_").replace(".", "_")

    ctx.actions.run(
        cmd_args(script_args, hidden = [cmd, srcs_artifact, out_artifact.as_output()]),
        env = env_vars,
        local_only = local_only,
        prefer_local = prefer_local,
        weight = value_or(ctx.attrs.weight, 1),
        allow_cache_upload = cacheable,
        allow_offline_output_cache = ctx.attrs.allow_offline_output_cache,
        category = category,
        identifier = identifier,
        no_outputs_cleanup = ctx.attrs.no_outputs_cleanup,
        always_print_stderr = ctx.attrs.always_print_stderr,
        **metadata_args,
    )

    return out_artifact

def _build_js_bundle(ctx: AnalysisContext, bundle_name_out: str, js_bundle_info: JsBundleInfo, named_output: str) -> JsBundleInfo:
    env_vars = {
        "DEPENDENCIES": cmd_args(js_bundle_info.dependencies_file),
        "JS_BUNDLE_NAME": cmd_args(js_bundle_info.bundle_name),
        "JS_BUNDLE_NAME_OUT": cmd_args(bundle_name_out),
        "JS_DIR": cmd_args(js_bundle_info.built_js),
        "MISC_DIR": cmd_args(js_bundle_info.misc),
        "PLATFORM": cmd_args(ctx.attrs._platform),
        "RELEASE": cmd_args("1" if ctx.attrs._is_release else ""),
        "RES_DIR": cmd_args(js_bundle_info.res),
        "SOURCEMAP": cmd_args(js_bundle_info.source_map),
    }
    has_content_based_path = ctx.attrs.has_content_based_path
    if ctx.attrs.rewrite_sourcemap:
        source_map_out = ctx.actions.declare_output("{}/source_map".format(named_output), has_content_based_path = has_content_based_path)
        env_vars["SOURCEMAP_OUT"] = cmd_args(source_map_out.as_output())
    else:
        source_map_out = js_bundle_info.source_map

    if ctx.attrs.rewrite_misc:
        misc_out = ctx.actions.declare_output("{}/misc".format(named_output), has_content_based_path = has_content_based_path)
        env_vars["MISC_OUT"] = cmd_args(misc_out.as_output())
    else:
        misc_out = js_bundle_info.misc

    if ctx.attrs.rewrite_deps_file:
        dependencies_out = ctx.actions.declare_output("{}/dependencies".format(named_output), has_content_based_path = has_content_based_path)
        env_vars["DEPENDENCIES_OUT"] = cmd_args(dependencies_out.as_output())
    else:
        dependencies_out = js_bundle_info.dependencies_file

    built_js = _run_genrule(ctx, "{}-js".format(named_output), env_vars, named_output)

    return JsBundleInfo(
        bundle_name = bundle_name_out,
        built_js = built_js,
        source_map = source_map_out,
        res = js_bundle_info.res,
        generic_assets = js_bundle_info.generic_assets,
        misc = misc_out,
        dependencies_file = dependencies_out,
    )

def _get_extra_providers(
    ctx: AnalysisContext, skip_resources: bool, initial_target: [ProviderCollection, Dependency], js_bundle_out: JsBundleInfo
) -> list[Provider]:
    providers = []
    android_resource_info = initial_target.get(AndroidResourceInfo)
    if android_resource_info:
        expect(js_bundle_out.generic_assets != None, "generic_assets must be set for Android js_bundle")
        assets = [js_bundle_out.built_js]
        if not skip_resources:
            assets.append(js_bundle_out.generic_assets)
        new_android_resource_info = AndroidResourceInfo(
            raw_target = ctx.label.raw_target(),
            aapt2_compile_output = None if skip_resources else android_resource_info.aapt2_compile_output,
            allow_strings_as_assets_resource_filtering = True,
            assets = assets,
            manifest_file = None,
            r_dot_java_package = None if skip_resources else android_resource_info.r_dot_java_package,
            res = None if skip_resources else android_resource_info.res,
            res_priority = android_resource_info.res_priority,
            text_symbols = None if skip_resources else android_resource_info.text_symbols,
            unused_resource_dep_validation_has_non_xml_resources = True,
        )
        providers.append(new_android_resource_info)
        providers.append(merge_android_packageable_info(ctx.label, ctx.actions, deps = [], resource_info = new_android_resource_info))

    providers += get_apple_resource_providers_for_js_bundle(ctx, js_bundle_out, ctx.attrs._platform, skip_resources)

    return providers

def js_bundle_genrule_impl(ctx: AnalysisContext) -> list[Provider]:
    sub_targets = {}
    for transform_profile in TRANSFORM_PROFILES:
        js_bundle = ctx.attrs.js_bundle[DefaultInfo].sub_targets[transform_profile]
        js_bundle_info = js_bundle[JsBundleInfo]
        bundle_name_out = get_bundle_name(ctx, js_bundle_info.bundle_name)

        js_bundle_out = _build_js_bundle(ctx, bundle_name_out, js_bundle_info, transform_profile)

        sub_targets[transform_profile] = [
            DefaultInfo(default_output = js_bundle_out.built_js),
            js_bundle_out,
        ] + _get_extra_providers(ctx, ctx.attrs.skip_resources, js_bundle, js_bundle_out)
        sub_targets["{}-misc".format(transform_profile)] = [DefaultInfo(default_output = js_bundle_out.misc)]
        sub_targets["{}-source_map".format(transform_profile)] = [DefaultInfo(default_output = js_bundle_out.source_map)]
        sub_targets["{}-dependencies".format(transform_profile)] = [DefaultInfo(default_output = js_bundle_out.dependencies_file)]
        sub_targets["{}-res".format(transform_profile)] = [DefaultInfo(default_output = js_bundle_out.res)]

    js_bundle_info = ctx.attrs.js_bundle[JsBundleInfo]
    bundle_name_out = get_bundle_name(ctx, js_bundle_info.bundle_name)
    js_bundle_out = _build_js_bundle(ctx, bundle_name_out, js_bundle_info, "default")

    sub_targets["dependencies"] = [DefaultInfo(default_output = js_bundle_out.dependencies_file)]
    sub_targets["misc"] = [DefaultInfo(default_output = js_bundle_out.misc)]
    sub_targets["source_map"] = [DefaultInfo(default_output = js_bundle_out.source_map)]
    sub_targets["res"] = [DefaultInfo(default_output = js_bundle_out.res)]
    default_info_out = DefaultInfo(
        default_output = js_bundle_out.built_js,
        sub_targets = sub_targets,
    )

    return [default_info_out, js_bundle_out] + _get_extra_providers(ctx, ctx.attrs.skip_resources, ctx.attrs.js_bundle, js_bundle_out)

js_bundle_genrule = prelude_rule(
    name = "js_bundle_genrule",
    impl = js_bundle_genrule_impl,
    docs = """
        `js_bundle_genrule` is a genrule that acts on a `js_bundle` and provides a bunch of
        additional environment variables for the genrule to use.
    """,
    examples = None,
    further = None,
    attrs = js_bundle_genrule_attrs(),
)
