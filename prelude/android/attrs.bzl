# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

load("@prelude//:validation_deps.bzl", "VALIDATION_DEPS_ATTR_NAME")
load("@prelude//android:configuration.bzl", "CPU_TRANSITION_ATTRS", "cpu_split_transition", "cpu_transition")
load("@prelude//decls:common.bzl", "TargetCpuType", "buck")
load("@prelude//decls:toolchains_common.bzl", "toolchains_common")
load("@prelude//transitions:constraint_overrides.bzl", "constraint_overrides")

def _manifest_apk_arg():
    return {
        "manifest": attrs.option(
            attrs.source(),
            default = None,
            doc = """
    Relative path to the Android manifest for the APK. The common
     case is that the manifest will be in the same directory as the
     rule, in which case this will simply be
     `'AndroidManifest.xml'`, but it can also reference
     an `android_manifest()` rule.

     Prefer using `manifest_skeleton`, which performs merging automatically.
     Exactly one of `manifest` and `manifest_skeleton` must be set.
""",
        ),
        "manifest_skeleton": attrs.option(
            attrs.source(),
            default = None,
            doc = """
    Relative path to the skeleton Android manifest for the APK.
     An `android_manifest()` will be created automatically to merge
     all manifests from libraries and resources going into the app.
     The common case is that the manifest will be in the same directory as the
     rule, in which case this will simply be
     `'AndroidManifest.xml'`.

     Exactly one of `manifest` and `manifest_skeleton` must be set.
""",
        ),
    }

def _deps_apk_arg():
    return {
        "deps": attrs.list(
            attrs.dep(),
            default = [],
            doc = """
    List of build targets whose corresponding compiled Java code,
     Android resources, and native libraries will be included in the APK.
     From the transitive closure of these dependencies, the outputs of rules of the following type will be included in the APK:
     * `android_library()`* `android_resource()`* `cxx_library()`* `groovy_library()`* `java_library()`* `java_binary()`* `prebuilt_jar()`* `prebuilt_native_library()`
""",
        ),
    }

def _manifest_arg():
    return {
        "manifest": attrs.option(
            attrs.source(),
            default = None,
            doc = """
    An optional [Android Manifest](http://developer.android.com/guide/topics/manifest/manifest-intro.html) for the to declare any permissions or intents it may need or want to handle. May either be a file or an `android_manifest()` target.
""",
        ),
    }

def _patches_system_module_arg():
    return {
        "patches_system_module": attrs.bool(
            default = False,
            doc = """
    Allow this target's sources to (re)declare packages that already exist
     in the `--system` module image (`java.base`). Needed under the JDK 9+
     module system when compiling stub sources for such packages (e.g.
     Android's `dalvik.system`), which otherwise fail with "package exists
     in another module: java.base". When set, the compiler is invoked with
     `--patch-module java.base=.`, folding this target's sources into that
     module. All sources in the compilation are patched in, so a target
     using this should only contain sources whose packages belong in the
     system module.
""",
        ),
    }

android_common = struct(
    manifest_apk_arg = _manifest_apk_arg,
    deps_apk_arg = _deps_apk_arg,
    manifest_arg = _manifest_arg,
    patches_system_module_arg = _patches_system_module_arg,
)

DexStore = ["raw", "raw_subdir", "jar", "xzs"]

DuplicateResourceBehaviour = ["allow_by_default", "ban_by_default"]

ExopackageMode = ["secondary_dex", "native_library", "resources", "modules", "arch64"]

JvmLanguage = ["java", "kotlin", "scala"]

PackageType = ["debug", "instrumented", "release", "test"]

RType = [
    "anim",
    "animator",
    "array",
    "attr",
    "bool",
    "color",
    "dimen",
    "drawable",
    "fraction",
    "font",
    "id",
    "integer",
    "interpolator",
    "layout",
    "menu",
    "navigation",
    "mipmap",
    "plurals",
    "raw",
    "string",
    "style",
    "styleable",
    "transition",
    "xml",
]

ResourceCompressionMode = ["disabled", "enabled", "enabled_strings_only", "enabled_with_strings_as_assets"]

SdkProguardType = ["default", "optimized", "none"]

GatoradePhase = ["early", "middle", "late"]

DISABLE_STRIPPING = read_root_config("android", "disable_stripping") in ("True", "true")

# Format is {"ovveride_name": {"re_cap_key": "re_cap_value"}}; for example:
#    {
#        "dynamic-listing": {
#            "platform": "riot",
#            "pool": "EUREKA_POOL",
#        },
#        "test-execution": {
#            "platform": "riot",
#            "pool": "EUREKA_POOL",
#        },
#    }
RE_CAPS = attrs.option(attrs.dict(key = attrs.string(), value = attrs.dict(key = attrs.string(), value = attrs.string())), default = None)

# Format is {"ovveride_name": "re_use_case"}; for example:
#     {
#         "dynamic-listing": "riot",
#         "test-execution": "riot",
#     }
RE_USE_CASE = attrs.option(attrs.dict(key = attrs.string(), value = attrs.string()), default = None)

# Format is {"ovveride_name": {"param_name": param_value}}; for example:
#    {
#        "remote_execution_policy": {"setup_preference_key": "some_json_string"},
#    }
META_INTERNAL_EXTRA_PARAMS = attrs.option(attrs.dict(key = attrs.string(), value = attrs.any()), default = None)

# Common attributes shared between android_binary and android_bundle rules
ANDROID_BINARY_BUNDLE_COMMON_ATTRS = (
    {
        "aapt2_keep_raw_values": attrs.bool(default = False, doc = ""),
        "aapt2_locale_filtering": attrs.bool(default = False, doc = ""),
        "aapt2_preferred_density": attrs.option(attrs.string(), default = None, doc = ""),
        "additional_aapt_params": attrs.list(attrs.string(), default = [], doc = ""),
        "allow_r_dot_java_in_secondary_dex": attrs.bool(default = False, doc = ""),
        "allowed_duplicate_resource_types": attrs.list(
            attrs.enum(RType),
            default = [],
            doc = "Similar to banned_duplicate_resource_types, but lists the types that are allowed to have duplicates. This should only be used if duplicate_resource_behavior is 'ban_by_default'.",
        ),
        "android_sdk_proguard_config": attrs.option(
            attrs.enum(SdkProguardType),
            default = None,
            doc = "The type of proguard configuration to use from the Android SDK. Options are 'default' to use the default config, 'optimized' to use the config with optimizations enabled, or 'none' to not use any standard configuration (you will need to supply your own version, otherwise your app will probably not work).",
        ),
        "application_module_blocklist": attrs.option(attrs.list(attrs.transition_dep(cfg = cpu_transition)), default = None, doc = ""),
        "application_module_configs": attrs.dict(
            key = attrs.string(),
            value = attrs.list(attrs.transition_dep(cfg = cpu_transition)),
            sorted = False,
            default = {},
            doc = "A map of module names to lists of targets, where the targets should seed the corresponding module. The seed targets and their exclusive dependencies are packaged into the APK in separate files to allow them to be loaded independently.",
        ),
        "application_module_dependencies": attrs.option(attrs.dict(key = attrs.string(), value = attrs.list(attrs.string()), sorted = False), default = None, doc = ""),
        "banned_duplicate_resource_types": attrs.list(
            attrs.enum(RType),
            default = [],
            doc = "If ['string'] is used, the build will break if multiple string resources with the same name are added in an app's Android Resources. Resource names are name-spaced by resource type, this does not enforce unique names between multiple resource types. AAPT does not enforce this, but you can prevent easy-to-introduce resource bugs by enabling this.",
        ),
        "build_config_values": attrs.list(attrs.string(), default = [], doc = "See the documentation on the values argument for android_build_config()."),
        "build_config_values_file": attrs.option(
            attrs.one_of(attrs.transition_dep(cfg = cpu_transition), attrs.source()),
            default = None,
            doc = "Overrides build_config_values and matching fields from each android_build_config() rule's values and values_file. Other package fields are retained.",
        ),
        "build_string_source_map": attrs.bool(default = False, doc = ""),
        "compiled_resource_apks": attrs.list(attrs.source(), default = [], doc = ""),
        "cpu_filters": attrs.list(
            attrs.enum(TargetCpuType),
            default = [],
            doc = """The CPU architecture filter applied to the final apk. Could be a subset of ARM, ARMV7, ARM64, X86, X86_64, MIPS. \
Note: If you set this parameter, you must setup your NDK, otherwise Buck build will fail.""",
        ),
        "default_module_manifest_skeleton": attrs.option(attrs.one_of(attrs.transition_dep(cfg = cpu_transition), attrs.source()), default = None, doc = ""),
        "defer_relink": attrs.bool(
            default = False,
            doc = """With the relinker, packages the native libraries before relinking them and relinks them in parallel, so steps that read the APK (such as Redex) need not wait for the relinks. \
The relinked libraries are the `[relinked_libs]` sub-target, listed in `[relinked_libs_manifest]`; an apk_genrule running `combine_apk_with_relinked_libs` swaps them in, and only its output carries the shipped libraries. \
This target's own APK and its `[native_libs]` directory (forwarded unchanged by every apk_genrule) hold the libraries before the relinker, while its `[unstripped_native_libraries*]`, `linker_*` sub-targets and `unstripped_shared_libraries` describe the libraries that ship, as do its per-library `[native_libs][<soname>]` sub-targets except under late Gatorade, where they are the relinks before it: symbolicate and debug a combined target, not this one, and read `[relinked_libs]` for the shipped libraries. \
Ignored without the relinker, with any exopackage mode, or with `native_library_pick_first`. \
Under BOLT, BTI link reports (`fbandroid.enable_bti_link_reports`) describe the relink before BOLT.""",
        ),
        "deps": attrs.list(
            attrs.split_transition_dep(cfg = cpu_split_transition),
            default = [],
            doc = """List of build targets whose corresponding compiled Java code, Android resources, and native libraries will be included in the APK. \
From the transitive closure of these dependencies, the outputs of rules of the following type will be included in the APK: \
android_library(), android_resource(), cxx_library(), groovy_library(), java_library(), java_binary(), prebuilt_jar(), prebuilt_native_library().""",
        ),
        "dex_compression": attrs.option(attrs.enum(DexStore), default = None, doc = ""),
        "disable_pre_dex": attrs.bool(default = False, doc = ""),
        "duplicate_class_checker_enabled": attrs.bool(default = False, doc = ""),
        "duplicate_resource_behavior": attrs.enum(
            DuplicateResourceBehaviour,
            default = "allow_by_default",
            doc = "If set to 'ban_by_default', duplicate resource definitions with the same type and name will cause the build to fail, unless they are excluded by allowed_duplicate_resource_types or duplicate_resource_whitelist. If set to 'allow_by_default' (the default), no duplicate resource checks will be performed. They can still be enabled on individual types by using banned_duplicate_resource_types.",
        ),
        "duplicate_resource_whitelist": attrs.option(
            attrs.source(),
            default = None,
            doc = "Either a build target or a path to a file containing a whitelist of resources that should be excluded from duplicate resource detection. Format is one resource per line with the type followed by the name, separated by a space. For example, 'string app_name'.",
        ),
        "enable_bootstrap_dexes": attrs.bool(default = False, doc = ""),
        "enable_relinker": attrs.bool(default = False, doc = ""),
        "exclude_duplicate_targets_do_not_use": attrs.list(attrs.dep(), default = [], doc = ""),
        "exopackage_modes": attrs.list(attrs.enum(ExopackageMode), default = [], doc = ""),
        "extra_filtered_resources": attrs.list(attrs.string(), default = [], doc = ""),
        "extra_no_compress_asset_extensions": attrs.list(attrs.string(), default = [], doc = ""),
        "extra_no_compress_asset_regex": attrs.option(attrs.string(), default = None, doc = ""),
        "extra_relinker_outputs": attrs.list(attrs.string(), default = [], doc = ""),
        # @oss-disable[end= ]: "gatorade_extra_args": attrs.list(attrs.arg(), default = [], doc = "Extra Gatorade cross-library step arguments"),
        # @oss-disable[end= ]: "gatorade_phases": attrs.set(attrs.enum(GatoradePhase), default = []),
        "ignore_aapt_proguard_config": attrs.bool(default = False, doc = "If true, the proguard config automatically generated by aapt will be ignored."),
        "include_build_info_file": attrs.bool(
            # @oss-disable[end= ]: default = True,
            default = False, # @oss-enable
            doc = "Bake `assets/BuildInfo.json` with the buck2 `BUCK_BUILD_ID` into the APK so an installed build can be resolved back to its buck2 build and source commit. On by default for non-release builds in fbsource; set this attribute to `False` to disable. The `apk_build` action invokes a toolchain-provided generator to write the file, so the baked id refreshes when the APK content changes and a no-op rebuild reuses a prior (equally valid) id.",
        ),
        "includes_vector_drawables": attrs.bool(
            default = False,
            doc = "When calling AAPT during the packaging process, pass the --no-version-vectors flag which ensures that any vector drawables which make use of the Android support library are backwards compatible with Android 4.4 and earlier.",
        ),
        "is_cacheable": attrs.bool(default = False, doc = ""),
        "is_voltron_language_pack_enabled": attrs.bool(default = False, doc = ""),
        "keystore": attrs.dep(doc = "A build target that identifies a keystore to use to sign the APK."),
        "locales": attrs.list(attrs.string(), default = [], doc = ""),
        "manifest": attrs.option(
            attrs.one_of(attrs.transition_dep(cfg = cpu_transition), attrs.source()),
            default = None,
            doc = """Relative path to the Android manifest for the APK. The common case is that the manifest will be in the same directory as the rule, in which case this will simply be 'AndroidManifest.xml', but it can also reference an `android_manifest()` rule. Prefer using `manifest_skeleton`, which performs merging automatically. Exactly one of `manifest` and `manifest_skeleton` must be set.""",
        ),
        "manifest_entries": attrs.dict(
            key = attrs.string(),
            value = attrs.any(),
            default = {},
            doc = "Insert values into the packaged AndroidManifest.xml file. Valid values are min_sdk_version, target_sdk_version, version_code, version_name, and debug_mode.",
        ),
        "manifest_skeleton": attrs.option(
            attrs.one_of(attrs.transition_dep(cfg = cpu_transition), attrs.source()),
            default = None,
            doc = """Relative path to the skeleton Android manifest for the APK. An `android_manifest()` will be created automatically to merge all manifests from libraries and resources going into the app. The common case is that the manifest will be in the same directory as the rule, in which case this will simply be 'AndroidManifest.xml'. Exactly one of `manifest` and `manifest_skeleton` must be set.""",
        ),
        "max_jvm_compiler_depth": attrs.option(
            attrs.int(),
            default = None,
            doc = "Optional inclusive limit on JavaCD/KotlinCD compiler depth through compilation classpaths, including source and source-only ABI compilation. Checked during analysis over the primary platform's Java packaging closure. The [jvm_compiler_depth] subtarget reports the depth and one longest chain. Class ABI extraction, KSP, other compiler backends, and dependencies hidden in generated sources/tools are not counted. None disables the limit.",
        ),
        "min_sdk_version": attrs.option(attrs.int(), default = None),
        "minimize_primary_dex_size": attrs.bool(default = False, doc = ""),
        "multidex_min_api": attrs.option(attrs.string(), default = None, doc = ""),
        "native_library_bolt_args": attrs.option(attrs.dict(key = attrs.string(), value = attrs.list(attrs.arg())), default = None, doc = ""),
        "native_library_merge_code_generator": attrs.option(attrs.exec_dep(), default = None, doc = ""),
        "native_library_merge_glue": attrs.option(attrs.split_transition_dep(cfg = cpu_split_transition), default = None, doc = ""),
        "native_library_merge_linker_args": attrs.option(attrs.dict(key = attrs.string(), value = attrs.list(attrs.arg())), default = None, doc = ""),
        "native_library_merge_linker_args_all": attrs.list(attrs.arg(), default = [], doc = "Extra linker arguments passed to all merged libraries."),
        "native_library_merge_map": attrs.option(
            attrs.dict(key = attrs.string(), value = attrs.list(attrs.regex()), sorted = False),
            default = None,
            doc = "An advanced option for apps with many native libraries.",
        ),
        "native_library_merge_non_asset_libs": attrs.bool(default = False, doc = ""),
        "native_library_merge_sequence": attrs.option(
            attrs.list(
                attrs.one_of(
                    attrs.tuple(attrs.string(), attrs.list(attrs.regex())),
                    attrs.list(attrs.tuple(attrs.string(), attrs.list(attrs.regex()))),
                ),
            ),
            default = None,
            doc = """Defines a prioritized sequence of instructions for merging native libraries. Sequence entries are tuples comprised of a merged library name and a list of regular expressions for which matching native library targets and their dependencies will be added to the merged library, unless an earlier entry already matched them. Merged library names must begin with `lib` and end with `.so`. This is an advanced option for apps with many native libraries.""",
        ),
        "native_library_merge_sequence_blocklist": attrs.option(
            attrs.list(attrs.regex()),
            default = None,
            doc = """Defines a list of regular expressions for which matching native library targets will not be merged by `native_library_merge_sequence`. Blocked targets' unblocked dependencies will still be merged.""",
        ),
        "native_library_pick_first": attrs.list(attrs.string(), default = [], doc = ""),
        "no_dx": attrs.list(
            attrs.dep(),
            default = [],
            doc = """List of dependency targets whose Java code should be excluded from the DEX files in the final APK. \
The classes from these targets will not be dexed, but their other resources will still be included. \
This might be useful to resolve duplicate class errors when multiple dependencies provide the same class. \
See [this document](https://www.internalfb.com/intern/staticdocs/confucius/docs/notes/dev/projects/android/resolving_duplicate_classes_with_no_dx/) \
for how to resolve duplicate classes using this attribute.""",
        ),
        "optimization_passes": attrs.int(default = 1, doc = ""),
        "package_asset_libraries": attrs.bool(default = False, doc = ""),
        "package_type": attrs.enum(
            PackageType,
            default = "debug",
            doc = "Determines whether ProGuard will be used when packaging the APK. Acceptable values for package_type are 'debug' and 'release'. The default value is 'debug', which indicates that ProGuard should not be used.",
        ),
        "package_validators": attrs.list(
            attrs.tuple(
                attrs.exec_dep(providers = [RunInfo]),
                attrs.list(attrs.arg(), default = []),
            ),
            default = [],
        ),
        "package_voltron_asset_libraries": attrs.bool(
            default = False,
            doc = "Similar global flag as package_asset_libraries to control voltron module libs specifically.",
        ),
        "packaged_locales": attrs.list(attrs.string(), default = [], doc = ""),
        "packaging_options": attrs.dict(key = attrs.string(), value = attrs.list(attrs.string()), default = {}, doc = ""),
        "post_filter_resources_cmd": attrs.option(attrs.arg(), default = None, doc = ""),
        "preinline_ir_sonames": attrs.list(attrs.string(), default = [], doc = "Sonames the pre-inline IR dump is restricted to (empty = all)."),
        "preprocess_java_classes_bash": attrs.option(attrs.arg(), default = None, doc = ""),
        "preprocess_java_classes_deps": attrs.list(attrs.dep(), default = [], doc = ""),
        "primary_dex_patterns": attrs.list(attrs.string(), default = [], doc = ""),
        "proguard_config": attrs.option(
            attrs.source(),
            default = None,
            doc = "Relative path to a ProGuard configuration file that will be passed via the -include flag when package_type is 'release'.",
        ),
        "proguard_jvm_args": attrs.list(attrs.string(), default = [], doc = ""),
        "relinker_extra_args": attrs.dict(
            key = attrs.string(),
            value = attrs.list(attrs.arg()),
            sorted = False,
            default = {},
            doc = "Per-library extra linker arguments passed when relinking, mapping sonames to lists of arguments.",
        ),
        "relinker_extra_args_all": attrs.list(attrs.arg(), default = [], doc = "Extra arguments passed when relinking all libraries."),
        "relinker_extra_deps": attrs.list(
            attrs.split_transition_dep(cfg = cpu_split_transition),
            default = [],
            doc = "Deps statically linked to every native lib by the relinker.",
        ),
        "relinker_whitelist": attrs.list(attrs.regex(), default = [], doc = ""),
        "resource_compression": attrs.enum(ResourceCompressionMode, default = "disabled", doc = ""),
        "resource_filter": attrs.list(attrs.string(), default = [], doc = ""),
        "resource_stable_ids": attrs.option(attrs.source(), default = None, doc = ""),
        "secondary_dex_weight_limit": attrs.option(attrs.int(), default = None, doc = ""),
        "shared_libraries_to_exclude": attrs.list(attrs.label(), default = [], doc = ""),
        "skip_crunch_pngs": attrs.option(attrs.bool(), default = None, doc = "If True, PNGs in the APK are not crushed by aapt."),
        "skip_proguard": attrs.bool(
            default = False,
            doc = "To produce a release build without running ProGuard set the skip_proguard flag to True. This will still cause ProGuard configuration files to be generated for use by other optimizers like Redex.",
        ),
        "trim_resource_ids": attrs.bool(default = False, doc = ""),
        "use_split_dex": attrs.bool(
            default = False,
            doc = """If this argument is True, Buck enables multidex support for the output APK. \
Multidex support enables your app to use multiple Dalvik Executable (DEX) bytecode files and thereby \
work around the 64K limit on the number of methods that can be referenced in a single DEX file.""",
        ),
        "xz_compression_level": attrs.int(default = 4, doc = ""),
        "_android_toolchain": toolchains_common.android(),
        "_cxx_toolchain": attrs.split_transition_dep(cfg = cpu_split_transition, default = "toolchains//:android-hack"),
        "_dex_toolchain": toolchains_common.dex(),
        "_exec_os_type": buck.exec_os_type_arg(),
        "_is_building_android_binary": attrs.default_only(attrs.bool(default = True)),
        "_java_toolchain": toolchains_common.java_for_android(),
        VALIDATION_DEPS_ATTR_NAME: attrs.set(attrs.transition_dep(cfg = cpu_transition), sorted = True, default = []),
    }
    | CPU_TRANSITION_ATTRS
    | buck.licenses_arg()
    | buck.labels_arg()
    | buck.contacts_arg()
)

ANDROID_BINARY_ATTRS = (
    ANDROID_BINARY_BUNDLE_COMMON_ATTRS
    | {
        "strip_libraries": attrs.bool(default = not DISABLE_STRIPPING),
    }
    | constraint_overrides.attributes
)
ANDROID_BUNDLE_ATTRS = ANDROID_BINARY_BUNDLE_COMMON_ATTRS | {
    "bundle_config_file": attrs.option(attrs.source(), default = None, doc = ""),
    "module_manifest_skeleton": attrs.dict(
        default = {},
        key = attrs.string(),
        sorted = False,
        value = attrs.one_of(attrs.transition_dep(cfg = cpu_transition), attrs.source()),
        doc = "",
    ),
    "module_manifests": attrs.option(attrs.transition_dep(cfg = cpu_transition), default = None, doc = ""),
    "use_derived_apk": attrs.bool(default = False),
}
