# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

# `native` is fine to use in the prelude for v2

# This is buck2's shim import. Any public symbols here will be available within
# **all** interpreted files.

load("@prelude//:build_variants.bzl", "build_variants_macro")
load("@prelude//:is_full_meta_repo.bzl", "is_full_meta_repo")
load("@prelude//:paths.bzl", "paths")
load("@prelude//:rules.bzl", __rules__ = "rules")
load("@prelude//android:cpu_filters.bzl", "ALL_CPU_FILTERS", "CPU_FILTER_FOR_DEFAULT_PLATFORM")
load(
    "@prelude//apple:apple_macro_layer.bzl",
    "apple_binary_macro_impl",
    "apple_bundle_macro_impl",
    "apple_library_for_distribution_macro_impl",
    "apple_library_macro_impl",
    "apple_metal_library_macro_impl",
    "apple_package_macro_impl",
    "apple_test_macro_impl",
    "apple_universal_executable_macro_impl",
    "apple_xcuitest_macro_impl",
    "prebuilt_apple_framework_macro_impl",
)
load("@prelude//apple:prebuilt_apple_xcframework_macro_impl.bzl", "prebuilt_apple_xcframework_macro_impl")
load("@prelude//apple/swift:swift_toolchain_macro_layer.bzl", "swift_toolchain_macro_impl")
load("@prelude//cxx:cxx_toolchain.bzl", "cxx_toolchain_inheriting_target_platform")
load("@prelude//cxx:cxx_toolchain_macro_layer.bzl", "cxx_toolchain_macro_impl")
load("@prelude//cxx:cxx_toolchain_types.bzl", _cxx = "cxx")
load("@prelude//erlang:erlang.bzl", _erlang_application = "erlang_application", _erlang_tests = "erlang_tests")
load("@prelude//python:toolchain.bzl", _python = "python")
load("@prelude//rust:rust_common.bzl", "rust_common_macro_wrapper")
load("@prelude//rust:rust_library.bzl", "rust_library_macro_wrapper")
load("@prelude//rust:with_workspace.bzl", "with_rust_workspace")
load("@prelude//user:all.bzl", _user_rules = "rules")
load(
    "@prelude//utils:buckconfig.bzl",
    _read_config = "read_config_with_logging",
    _read_root_config = "read_root_config_with_logging",
    log_buckconfigs = "LOG_BUCKCONFIGS",
)
load("@prelude//utils:expect.bzl", "expect")
load("@prelude//utils:selects.bzl", "selects")

def __struct_to_dict(s):
    vals = {}
    for name in dir(s):
        vals[name] = getattr(s, name)
    return vals

def _tp2_constraint(project, version):
    """
    Return the target configuration constraint to use for the given project
    version
    """

    return "ovr_config//third-party/{}/constraints:{}".format(project, version)

def _tp2_constraint_multi(versions):
    """
    Return the `select` key rule name which corresponds to the given tp2 project
    versions.
    """

    expect(len(versions) >= 1, str(versions))

    # If there's only a single project/version pair, then just return the its
    # pre-defined constraint value rule name.
    if len(versions) == 1:
        (project, version) = versions.items()[0]
        return _tp2_constraint(project, version)

    # Otherwise, generate a `config_setting` to combine the constraint value
    # rules for all the project/version pairs.
    name = "-".join(["_tp2_constraints_"] + ["{}-{}".format(p, v) for p, v in sorted(versions.items())])
    if not rule_exists(name):
        __rules__["config_setting"](
            name = name,
            constraint_values = [_tp2_constraint(p, v) for p, v in versions.items()],
        )
    return ":" + name

def _extract_versions(constraints):
    """
    Convert v1-style version constraints to a v2-compatible config settings.

    The constraints are normally of the form:
    `{"python": "3.8"}`.
    """

    versions = {}

    # Since the constraints will be duplicated for each fbcode "platform", do
    # some initial work to de-duplicate them here, by extracting just the
    # project and version and verify we get just a single reduced result.
    for project, version in constraints.items():
        expect(project not in versions or version == versions[project])
        versions[project] = version

    return versions

def _versioned_param_to_select(items, default = None):
    """
    Convert a v1-style `versioned_*` param to a `select` map.

    Parameters:
    - items: A list of 2-tuples of a list of version constraints to match and
          the values to use in case of match.
    """

    if items == None:
        return None

    # TODO(agallagher): Remove once we move to a `uquery` based TD.
    if read_root_config("fbcode", "cquery_td") == "true":
        return None

    # Special case a form of "empty" constraints that `buckify_tp2` may
    # generate in tp2 TARGETS.
    if len(items) == 1 and not items[0][0]:
        return items[0][1]

    select_map = {}

    # If a default is provided add that.
    if default != None:
        select_map["DEFAULT"] = default

    # Convert v1 tp2-style versioned_* params to their analogous v2 select
    # constraint maps.
    for constraints, item in items:
        versions = _extract_versions(constraints)
        select_map[_tp2_constraint_multi(versions)] = item

    if not select_map:
        return None

    return select(select_map)

def _concat(*items):
    """
    Concatenate non-`None` items and return result.
    """
    res = None

    for item in items:
        if item == None:
            continue
        if res == None:
            res = item
        elif type(res) == type({}) and type(item) == type({}):
            new_res = {}
            new_res.update(res)
            new_res.update(item)
            res = new_res
        else:
            res += item  # buildifier: disable=dict-concatenation

    return res

def _at_most_one(*items):
    """
    Return a non-`None` value if it exists.  Fail if more that one non-`None`
    exists.
    """

    res = None

    for item in items:
        if item == None:
            continue
        expect(res == None)
        res = item

    return res

def _get_valid_cpu_filters(cpu_filters: [list[str], None]) -> list[str]:
    if read_root_config("buck2", "android_force_single_default_cpu") in ("True", "true"):
        return [CPU_FILTER_FOR_DEFAULT_PLATFORM]

    cpu_abis_config_string = read_root_config("ndk", "cpu_abis")
    if cpu_abis_config_string:
        cpu_abis = [v.strip() for v in cpu_abis_config_string.split(",")]
        for cpu_abi in cpu_abis:
            if cpu_abi not in ALL_CPU_FILTERS:
                fail("Entries in ndk.cpu_abis must be one of {}, but {} is not".format(ALL_CPU_FILTERS, cpu_abi))
    else:
        cpu_abis = ALL_CPU_FILTERS

    cpu_filters = cpu_filters or ALL_CPU_FILTERS

    return [cpu_filter for cpu_filter in cpu_filters if cpu_filter in cpu_abis]

def _android_aar_macro_stub(cpu_filters = None, **kwargs):
    __rules__["android_aar"](cpu_filters = _get_valid_cpu_filters(cpu_filters), **kwargs)

def _convert_kotlin_compiler_plugins(kotlin_compiler_plugins):
    if type(kotlin_compiler_plugins) == type(select({})):
        return native.select_map(kotlin_compiler_plugins, _convert_kotlin_compiler_plugins)
    if type(kotlin_compiler_plugins) == type({}):
        return [(key, value) for key, value in kotlin_compiler_plugins.items()]
    else:
        return kotlin_compiler_plugins

def _kotlin_library_macro_stub(kotlin_compiler_plugins = {}, **kwargs):
    __rules__["kotlin_library"](kotlin_compiler_plugins = _convert_kotlin_compiler_plugins(kotlin_compiler_plugins), **kwargs)

def _kotlin_test_macro_stub(kotlin_compiler_plugins = {}, **kwargs):
    __rules__["kotlin_test"](kotlin_compiler_plugins = _convert_kotlin_compiler_plugins(kotlin_compiler_plugins), **kwargs)

def _android_library_macro_stub(kotlin_compiler_plugins = {}, **kwargs):
    __rules__["android_library"](kotlin_compiler_plugins = _convert_kotlin_compiler_plugins(kotlin_compiler_plugins), **kwargs)

def _robolectric_test_macro_stub(kotlin_compiler_plugins = {}, **kwargs):
    __rules__["robolectric_test"](kotlin_compiler_plugins = _convert_kotlin_compiler_plugins(kotlin_compiler_plugins), **kwargs)

def _android_binary_macro_stub(allow_r_dot_java_in_secondary_dex = False, cpu_filters = None, primary_dex_patterns = [], **kwargs):
    if not allow_r_dot_java_in_secondary_dex:
        primary_dex_patterns = primary_dex_patterns + [
            "/R^",
            "/R$",
            # Pin this to the primary for apps with no primary dex classes.
            "^com/facebook/buck_generated/AppWithoutResourcesStub^",
        ]

    # TODO: T218493860 Accept `select` for `cpu_filters` and apply the same logic as for non-select cases
    __rules__["android_binary"](
        allow_r_dot_java_in_secondary_dex = allow_r_dot_java_in_secondary_dex,
        cpu_filters = cpu_filters if isinstance(cpu_filters, Select) else _get_valid_cpu_filters(cpu_filters),
        primary_dex_patterns = primary_dex_patterns,
        **kwargs,
    )

def _android_bundle_macro_stub(cpu_filters = None, **kwargs):
    __rules__["android_bundle"](
        # TODO: T218493860 Accept `select` for `cpu_filters` and apply the same logic as for non-select cases
        cpu_filters = cpu_filters if isinstance(cpu_filters, Select) else _get_valid_cpu_filters(cpu_filters),
        **kwargs,
    )

def _android_instrumentation_apk_macro_stub(cpu_filters = None, primary_dex_patterns = [], **kwargs):
    primary_dex_patterns = primary_dex_patterns + [
        "/R^",
        "/R$",
        # Pin this to the primary for apps with no primary dex classes.
        "^com/facebook/buck_generated/AppWithoutResourcesStub^",
    ]
    __rules__["android_instrumentation_apk"](cpu_filters = _get_valid_cpu_filters(cpu_filters), primary_dex_patterns = primary_dex_patterns, **kwargs)

# export_file src defaults to name, despite being string vs source, so adjust it in the macros
def _build_variants_macro_stub(**kwargs):
    build_variants_macro(build_variants_rule = __rules__["build_variants"], **kwargs)

def _export_file_macro_stub(name, src = None, **kwargs):
    __rules__["export_file"](name = name, src = name if src == None else src, **kwargs)

def _prebuilt_cxx_library_macro_stub(
    exported_preprocessor_flags = None,
    versioned_exported_preprocessor_flags = None,
    exported_lang_preprocessor_flags = None,
    versioned_exported_lang_preprocessor_flags = None,
    static_lib = None,
    versioned_static_lib = None,
    static_pic_lib = None,
    versioned_static_pic_lib = None,
    shared_lib = None,
    versioned_shared_lib = None,
    header_dirs = None,
    versioned_header_dirs = None,
    **kwargs,
):
    __rules__["prebuilt_cxx_library"](
        exported_preprocessor_flags = _concat(
            exported_preprocessor_flags,
            _versioned_param_to_select(versioned_exported_preprocessor_flags),
        ),
        exported_lang_preprocessor_flags = _concat(
            exported_lang_preprocessor_flags,
            _versioned_param_to_select(versioned_exported_lang_preprocessor_flags),
        ),
        static_lib = selects.apply_n(
            [static_lib, selects.apply(versioned_static_lib, _versioned_param_to_select)],
            _at_most_one,
        ),
        static_pic_lib = selects.apply_n(
            [static_pic_lib, selects.apply(versioned_static_pic_lib, _versioned_param_to_select)],
            _at_most_one,
        ),
        shared_lib = selects.apply_n(
            [shared_lib, selects.apply(versioned_shared_lib, _versioned_param_to_select)],
            _at_most_one,
        ),
        header_dirs = selects.apply_n(
            [header_dirs, selects.apply(versioned_header_dirs, _versioned_param_to_select)],
            _at_most_one,
        ),
        **kwargs,
    )

def _python_library_macro_stub(srcs = None, versioned_srcs = None, resources = None, versioned_resources = None, **kwargs):
    __rules__["python_library"](
        srcs = _concat(srcs, _versioned_param_to_select(versioned_srcs, default = None)),
        resources = _concat(resources, _versioned_param_to_select(versioned_resources, default = None)),
        **kwargs,
    )

def _versioned_alias_macro_stub(versions = {}, **kwargs):
    project = paths.basename(package_name())
    __rules__["alias"](actual = select({_tp2_constraint(project, version): actual for version, actual in versions.items()}), **kwargs)

def _configured_alias_macro_stub(
    name,
    actual,
    platform,
    # Whether to fallback to a unconfigured `alias` if `platform` is `None`.
    fallback_to_unconfigured_alias = False,
    **kwargs,
):
    pred = lambda platform: platform != None or not fallback_to_unconfigured_alias
    __rules__["configured_alias"](
        name = name,
        # `actual` needs to be a pair of target + platform, as that's the format
        # expected by the `configured_dep()` field
        # Use a select map to make this thing `None` if `platform` is `None`.
        configured_actual = selects.apply(
            platform,
            lambda platform: (actual, platform) if pred(platform) else None,
        ),
        # Make sure that exactly one of `configured_actual` or `fallback_actual` is set
        fallback_actual = selects.apply(
            platform,
            lambda platform: None if pred(platform) else actual,
        ),
        # Unused.
        actual = actual,
        platform = platform,
        **kwargs,
    )

def _apple_bundle_macro_stub(**kwargs):
    apple_bundle_macro_impl(apple_bundle_rule = __rules__["apple_bundle"], **kwargs)

def _apple_watchos_bundle_macro_stub(**kwargs):
    apple_bundle_macro_impl(apple_bundle_rule = __rules__["apple_watchos_bundle"], **kwargs)

def _apple_macos_bundle_macro_stub(**kwargs):
    apple_bundle_macro_impl(apple_bundle_rule = __rules__["apple_macos_bundle"], **kwargs)

def _apple_test_macro_stub(**kwargs):
    apple_test_macro_impl(apple_test_rule = __rules__["apple_test"], **kwargs)

def _apple_xcuitest_macro_stub(**kwargs):
    apple_xcuitest_macro_impl(apple_xcuitest_rule = __rules__["apple_xcuitest"], **kwargs)

def _apple_binary_macro_stub(**kwargs):
    apple_binary_macro_impl(apple_binary_rule = __rules__["apple_binary"], apple_universal_executable = __rules__["apple_universal_executable"], **kwargs)

def _apple_library_macro_stub(**kwargs):
    apple_library_macro_impl(apple_library_rule = __rules__["apple_library"], **kwargs)

def _apple_metal_library_macro_stub(**kwargs):
    apple_metal_library_macro_impl(apple_metal_library_rule = __rules__["apple_metal_library"], **kwargs)

def _apple_library_for_distribution_macro_stub(**kwargs):
    apple_library_for_distribution_macro_impl(apple_library_for_distribution_rule = __rules__["apple_library_for_distribution"], **kwargs)

def _apple_package_macro_stub(**kwargs):
    apple_package_macro_impl(apple_package_rule = __rules__["apple_package"], apple_ipa_package_rule = __rules__["apple_ipa_package"], **kwargs)

def _apple_universal_executable_macro_stub(**kwargs):
    apple_universal_executable_macro_impl(apple_universal_executable_rule = __rules__["apple_universal_executable"], **kwargs)

def _swift_toolchain_macro_stub(**kwargs):
    rule = __rules__["swift_toolchain"]

    swift_toolchain_macro_impl(swift_toolchain_rule = rule, **kwargs)

def _cxx_toolchain_macro_stub(**kwargs):
    if is_full_meta_repo():
        cache_links = kwargs.get("cache_links")
        kwargs["cache_links"] = select({
            "DEFAULT": cache_links,
            "ovr_config//platform/execution/constraints:execution-platform-transitioned": True,
        })
    cxx_toolchain_macro_impl(cxx_toolchain_rule = cxx_toolchain_inheriting_target_platform, **kwargs)

def _cxx_toolchain_override_macro_stub(**kwargs):
    cxx_toolchain_macro_impl(cxx_toolchain_rule = _user_rules["cxx_toolchain_override"], **kwargs)

def _erlang_application_macro_stub(**kwargs):
    _erlang_application(erlang_app_rule = __rules__["erlang_app"], erlang_app_includes_rule = __rules__["erlang_app_includes"], **kwargs)

def _erlang_tests_macro_stub(**kwargs):
    _erlang_tests(erlang_app_rule = __rules__["erlang_app"], erlang_test_rule = __rules__["erlang_test"], **kwargs)

def _rust_library_macro_stub(**kwargs):
    rust_library = rust_common_macro_wrapper(__rules__["rust_library"])
    rust_library = rust_library_macro_wrapper(rust_library)
    rust_library(**kwargs)

def _rust_binary_macro_stub(**kwargs):
    rust_binary = rust_common_macro_wrapper(__rules__["rust_binary"])
    rust_binary(**kwargs)

def _rust_test_macro_stub(**kwargs):
    rust_test = rust_common_macro_wrapper(__rules__["rust_test"])
    rust_test(**kwargs)

def _prebuilt_apple_framework_macro_stub(**kwargs):
    prebuilt_apple_framework_macro_impl(prebuilt_apple_framework_rule = __rules__["prebuilt_apple_framework"], **kwargs)

def _prebuilt_apple_xcframework_macro_stub(**kwargs):
    prebuilt_apple_xcframework_macro_impl(
        filegroup_rule = __rules__["filegroup"], genrule = __rules__["genrule"], prebuilt_apple_framework_rule = __rules__["prebuilt_apple_framework"], **kwargs
    )

# TODO(cjhopman): These macro wrappers should be handled in prelude/rules.bzl.
# Probably good if they were defined to take in the base rule that
# they are wrapping and return the wrapped one.
__extra_rules__ = {
    "android_aar": _android_aar_macro_stub,
    "android_binary": _android_binary_macro_stub,
    "android_bundle": _android_bundle_macro_stub,
    "android_instrumentation_apk": _android_instrumentation_apk_macro_stub,
    "android_library": _android_library_macro_stub,
    "apple_binary": _apple_binary_macro_stub,
    "apple_bundle": _apple_bundle_macro_stub,
    "apple_library": _apple_library_macro_stub,
    "apple_library_for_distribution": _apple_library_for_distribution_macro_stub,
    "apple_macos_bundle": _apple_macos_bundle_macro_stub,
    "apple_metal_library": _apple_metal_library_macro_stub,
    "apple_package": _apple_package_macro_stub,
    "apple_test": _apple_test_macro_stub,
    "apple_universal_executable": _apple_universal_executable_macro_stub,
    "apple_watchos_bundle": _apple_watchos_bundle_macro_stub,
    "apple_xcuitest": _apple_xcuitest_macro_stub,
    "build_variants": _build_variants_macro_stub,
    "configured_alias": _configured_alias_macro_stub,
    "cxx_toolchain": _cxx_toolchain_macro_stub,
    "cxx_toolchain_override": _cxx_toolchain_override_macro_stub,
    "erlang_application": _erlang_application_macro_stub,
    "erlang_tests": _erlang_tests_macro_stub,
    "export_file": _export_file_macro_stub,
    "kotlin_library": _kotlin_library_macro_stub,
    "kotlin_test": _kotlin_test_macro_stub,
    "prebuilt_apple_framework": _prebuilt_apple_framework_macro_stub,
    "prebuilt_apple_xcframework": _prebuilt_apple_xcframework_macro_stub,
    "prebuilt_cxx_library": _prebuilt_cxx_library_macro_stub,
    "python_library": _python_library_macro_stub,
    "robolectric_test": _robolectric_test_macro_stub,
    "rust_binary": _rust_binary_macro_stub,
    "rust_library": _rust_library_macro_stub,
    "rust_test": _rust_test_macro_stub,
    "rust_with_workspace": with_rust_workspace,
    "swift_toolchain": _swift_toolchain_macro_stub,
    "versioned_alias": _versioned_alias_macro_stub,
}

__overridden_builtins__ = (
    {
        "read_config": _read_config,
        "read_root_config": _read_root_config,
    }
    if log_buckconfigs
    else {}
)

__shimmed_native__ = __struct_to_dict(__buck2_builtins__)
__shimmed_native__.update(__overridden_builtins__)
__shimmed_native__.update(__rules__)
__shimmed_native__.update(_user_rules)

# Should come after the rules which are macro overridden
__shimmed_native__.update(__extra_rules__)
__shimmed_native__.update({"cxx": _cxx, "python": _python})

native = struct(**__shimmed_native__)
