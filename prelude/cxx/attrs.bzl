# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

load("@prelude//:attrs_validators.bzl", "validation_common")
load("@prelude//android:configuration.bzl", "is_building_android_binary_attr")
load("@prelude//android:native_build_commands.bzl", "EMIT_NATIVE_BUILD_COMMANDS")
load("@prelude//apple:apple_common.bzl", "apple_common")
load("@prelude//cxx:cuda.bzl", "CudaCompileStyle")
load("@prelude//cxx:cxx_flags.bzl", "CxxFlagsInfo")
load("@prelude//cxx:headers.bzl", "CPrecompiledHeaderInfo", "HeaderMode")
load("@prelude//cxx:link_groups_types.bzl", "LINK_GROUP_MAP_ATTR")
load(
    "@prelude//decls:common.bzl",
    "CxxRuntimeType",
    "CxxSourceType",
    "DefaultDepsMode",
    "HeadersAsRawHeadersMode",
    "IncludeType",
    "RawHeadersAsHeadersMode",
    "RuntimeDependencyHandling",
    "buck",
)
load("@prelude//decls:native_common.bzl", "native_common")
load("@prelude//decls:third_party_common.bzl", "third_party_common")
load("@prelude//decls:toolchains_common.bzl", "toolchains_common")
load("@prelude//linking:execution_preference.bzl", "link_execution_preference_attr")
load("@prelude//linking:link_info.bzl", "ArchiveContentsType", "LinkOrdering")
load("@prelude//linking:types.bzl", "LinkPreference", "Linkage")
load("@prelude//transitions:constraint_overrides.bzl", "constraint_overrides")

def _srcs_arg():
    return {
        "srcs": attrs.list(
            attrs.one_of(attrs.source(), attrs.tuple(attrs.source(), attrs.list(attrs.arg()))),
            default = [],
            doc = """
    The set of C, C++, Objective-C, Objective-C++, or assembly source files
     to be preprocessed, compiled, and assembled by this
     rule. We determine which stages to run on each input source based on its file extension. See the
     [GCC documentation](https://gcc.gnu.org/onlinedocs/gcc/Overall-Options.html) for more detail on how file extensions are interpreted. Each element can be either a string
     specifying a source file (e.g. `''`) or a tuple of
     a string specifying a source file and a list of compilation flags
     (e.g. `('', ['-Wall', '-Werror'])` ). In the latter case the specified flags will be used in addition to the rule's other
     flags when preprocessing and compiling that file (if applicable).
""",
        ),
    }

def _deps_arg():
    return {
        "deps": attrs.list(
            attrs.dep(),
            default = [],
            doc = """
    Other rules that list `srcs` from which this rule imports.
""",
        ),
    }

def _default_deps_arg(default = "deps"):
    return {
        "default_deps": attrs.enum(
            DefaultDepsMode,
            default = default,
            doc = """
    Whether to include the default deps from the toolchain, and where to include them (deps, exported_deps, etc).
""",
        ),
    }

def _supported_platforms_regex_arg():
    return {
        "supported_platforms_regex": attrs.option(
            attrs.regex(),
            default = None,
            doc = """
    Deprecated: do not set this attribute in new code. Instead, use `target_compatible_with` to
     restrict the platforms a target can be built for.

     If present, an un-anchored regex (in java.util.regex.Pattern syntax) that matches all platforms
     that this library supports. It will not be built for other platforms.
""",
        ),
    }

def _headers_arg():
    return {
        "headers": attrs.named_set(
            attrs.source(),
            sorted = True,
            default = [],
            doc = """
    The set of header files that are made available for inclusion to the source files in this
     target. These should be specified as either a list
     of header files or a dictionary of header names to header files. The header name can contain
     forward slashes (`/`). The headers can be included with `#include
     "$HEADER_NAMESPACE/$HEADER_NAME"` or `#include <$HEADER_NAMESPACE/$HEADER_NAME>` , where `$HEADER_NAMESPACE` is the value of the target's `header_namespace`  attribute, and `$HEADER_NAME` is the header name if specified, and the filename
     of the header file otherwise. See `header_namespace` for more information.
""",
        ),
    }

def _exported_headers_arg():
    return {
        "exported_headers": attrs.named_set(
            attrs.source(),
            sorted = True,
            default = [],
            doc = """
    The set of header files that are made available for inclusion to the source files in the
     target and all targets that transitively depend on it. These should be specified as either a list
     of header files or a dictionary of header names to header files. The headers can be included
     with `#include "$HEADER_NAMESPACE/$HEADER_NAME"` or `#include
     <$HEADER_NAMESPACE/$HEADER_NAME>`, where `$HEADER_NAMESPACE` is the value
     of the target's `header_namespace` attribute, and `$HEADER_NAME` is the
     header name if specified, and the filename of the header file otherwise. Note that the header name
     can contain forward slashes (`/`). See `header_namespace` for more
     information.
""",
        ),
    }

def _exported_header_style_arg():
    return {
        "exported_header_style": attrs.enum(
            IncludeType,
            default = "local",
            doc = """
    How dependents should include exported headers from this rule. Can be either `local`
     (e.g. `-I`) or `system` (e.g. `-isystem`).
""",
        ),
    }

def _header_namespace_arg():
    return {
        "header_namespace": attrs.option(
            attrs.string(),
            default = None,
            doc = """
    A path prefix when including headers of this target. Defaults to the path from the root of the
     repository to the directory where this target is defined. Can
     contain forward slashes (`/`), but cannot start with one. See `headers` for
     more information.
""",
        ),
    }

def _preprocessor_flags_arg():
    return {
        "preprocessor_flags": attrs.list(
            attrs.arg(),
            default = [],
            doc = """
    Flags to use when preprocessing any of the above sources (which require preprocessing).
""",
        ),
    }

def _lang_preprocessor_flags_arg():
    return {
        "lang_preprocessor_flags": attrs.dict(
            key = attrs.enum(CxxSourceType),
            value = attrs.list(attrs.arg()),
            sorted = False,
            default = {},
            doc = """
    Language-specific preprocessor flags. These should be specified as a map of C-family language short
     names to lists of flags and is used to target flags to sources files for a specific language in the
     C-family (C, C++, assembler, etc.). The keys in the map can be:
     * `c` for C
    * `c++` for C++
    * `objective-c` for Objective-C
    * `objective-c++` for Objective-C++
    * `cuda` for Cuda
    * `assembler-with-cpp` for Assembly
    * `asm-with-cpp` for ASM
""",
        ),
    }

def _exported_lang_preprocessor_flags_arg():
    return {
        "exported_lang_preprocessor_flags": attrs.dict(
            key = attrs.enum(CxxSourceType),
            value = attrs.list(attrs.arg()),
            sorted = False,
            default = {},
            doc = """
    Just as `lang_preprocessor_flags`, but these flags also apply to
     rules that transitively depend on this rule.
""",
        ),
    }

def _exported_preprocessor_flags_arg(exported_preprocessor_flags_type):
    return {
        "exported_preprocessor_flags": exported_preprocessor_flags_type,
    }

def _compiler_flags_arg():
    return {
        "compiler_flags": attrs.list(
            attrs.arg(),
            default = [],
            doc = """
    Flags to use when compiling any of the above sources (which require compilation).
""",
        ),
    }

def _lang_compiler_flags_arg():
    return {
        "lang_compiler_flags": attrs.dict(
            key = attrs.enum(CxxSourceType),
            value = attrs.list(attrs.arg()),
            sorted = False,
            default = {},
            doc = """
    Language-specific compiler flags. These should be specified as a map of C-family language short
     names to lists of flags and is used to target flags to sources files for a specific language in the
     C-family (C, C++, assembler, etc.). The keys in the map can be:
     * `cpp-output` for C
    * `c++-cpp-output` for C++
    * `objective-c-cpp-output` for Objective-C
    * `objective-c++-cpp-output` for Objective-C++
    * `cuda-cpp-output` for Cuda
    * `assembler` for Assembly
    * `asm` for ASM
""",
        ),
    }

def _linker_extra_outputs_arg():
    return {
        "linker_extra_outputs": attrs.list(
            attrs.string(),
            default = [],
            doc = """
    Declares extra outputs that the linker emits. These identifiers can be used in
     `$(output ...)` macros in `linker_flags` to interpolate the output path
     into the linker command line. Useful for custom linkers that emit extra output files.
""",
        ),
    }

def _linker_flags_arg():
    return {
        "linker_flags": attrs.list(
            attrs.arg(anon_target_compatible = True),
            default = [],
            doc = """
    Flags to add to the linker command line whenever the output from this
     rule is used in a link operation, such as linked into an executable
     or a shared library.
""",
        ),
    }

def _local_linker_flags_arg():
    return {
        "local_linker_flags": attrs.list(
            attrs.arg(),
            default = [],
            doc = """
    Flags to add to the linker command line whenever the output from this
     rule is used in a link operation *driven by this rule* (e.g. when this
     rule links a shared library, but *not* when the output is linked into a
     shared library by another rule's link group links).
""",
        ),
    }

# TODO(christylee): remove this arg once link group map migrates to its own rule
def _local_linker_script_flags_arg():
    return {
        "local_linker_script_flags": attrs.list(
            attrs.arg(),
            default = [],
            doc = """
    Linker script lags to add to the linker command line whenever the output
     from this rule is used in a link operation *driven by this rule*. Used
     by rules that need to treat linker script flags different from normal
     linker flags.
""",
        ),
    }

def _exported_linker_flags_arg():
    return {
        "exported_linker_flags": attrs.list(
            attrs.arg(anon_target_compatible = True),
            default = [],
            doc = """
    Flags to add to the linker command line when the output from this
     rule, or the output from any rule that transitively depends on this
     rule, is used in a link operation.
""",
        ),
    }

def _exported_post_linker_flags_arg():
    return {
        "exported_post_linker_flags": attrs.list(
            attrs.arg(anon_target_compatible = True),
            default = [],
            doc = """
    Flags to add to the linker command line when the output from this
     rule, or the output from any rule that transitively depends on this
     rule, is used in a link operation—with the additional feature
     that these flags are guaranteed to be placed *after* the compiled
     object (`.o`) files on the linker command line.
""",
        ),
    }

def _precompiled_header_arg():
    return {
        "precompiled_header": attrs.option(
            attrs.source(),
            default = None,
            doc = """
    Path to a `cxx_precompiled_header` to use when compiling this rule's sources. The precompiled header (PCH) is built on-demand, using
     compiler flags matching those used in this rule's compile jobs. This is to ensure
     compatibility between this rule and the PCH. Also, this rule will inherit additional
     `deps` from the PCH rule, and as a result, additional include paths as well
     (e.g. `-I`, `-isystem`, `-iquote` path lists,
     and framework paths specified with `-F`).
""",
        ),
    }

def _force_static(force_static_type):
    return {
        "force_static": force_static_type,
    }

def _reexport_all_header_dependencies_arg():
    return {
        "reexport_all_header_dependencies": attrs.option(
            attrs.bool(),
            default = None,
            doc = """
    Deprecated: do not set this attribute in new code. Instead, list dependencies whose
     headers should be re-exported in `exported_deps`. Using this attribute is generally
     bad for build speed.

     Whether to automatically re-export the exported headers of all dependencies.

     When this is set to false, only exported headers from
     `exported_deps` are re-exported.
""",
        ),
    }

def _exported_deps_arg():
    return {
        "exported_deps": attrs.list(
            attrs.dep(),
            default = [],
            doc = """
    Dependencies that will also appear to belong to any rules that depend on this
     one. This has two effects:
     * Exported dependencies will also be included in the link line of
     dependents of this rules, but normal dependencies will not.
    * When `reexport_all_header_dependencies = False`, only exported
     headers of the rules specified here are re-exported.
""",
        ),
    }

def _supports_merged_linking():
    return {
        "supports_merged_linking": attrs.option(
            attrs.bool(),
            default = None,
            doc = """
    Whether this rule supports building with the merged linking strategy when building for non-native
     binaries (e.g. when using `.buckconfig`
    s `merged` setting).
""",
        ),
    }

def _raw_headers_arg():
    return {
        "raw_headers": attrs.set(
            attrs.source(),
            sorted = True,
            default = [],
            doc = """
    The set of header files that can be used for inclusion to the source files in the target and all
     targets that transitively depend on it. Buck doesn't add raw headers to the search path of a
     compiler/preprocessor automatically.
     `include_directories` and `public_include_directories` are the recommended
     way to add raw headers to the search path (they will be added via `-I`).
     `compiler_flags`, `preprocessor_flags` and `exported_preprocessor_flags`
     can also be used to add such raw headers to the search path if inclusion via `-isystem` or
     `-iquote` is needed.
     `raw_headers` cannot be used together with `headers` or `exported_headers` in the same target.
""",
        ),
    }

def _supports_stripping():
    return {
        "supports_stripping": attrs.bool(
            default = True,
            doc = """
    Whether this rule supports stripping outputs via `strip` command for exporting.
""",
        ),
    }

def _raw_headers_as_headers_mode_arg():
    return {
        "raw_headers_as_headers_mode": attrs.option(
            attrs.enum(RawHeadersAsHeadersMode),
            default = None,
            doc = """
    Controls whether raw_headers and *include_directories attributes should be automatically
    converted to headers and symlink trees and/or header maps via headers. Only has an effect if
    the cxx_toolchain has explicitly opted into supporting this behavior via a non-default value,
    even if the value is disabled.
""",
        ),
    }

def _include_directories_arg():
    return {
        "include_directories": attrs.set(
            attrs.string(),
            sorted = True,
            default = [],
            doc = """
    A list of include directories (with `raw_headers`) to be added to the compile command for compiling this target
     (via `-I`).
     An include directory is relative to the current package.
""",
        ),
    }

def _public_include_directories_arg():
    return {
        "public_include_directories": attrs.set(
            attrs.string(),
            sorted = True,
            default = [],
            doc = """
    A list of include directories (with `raw_headers`) to be added to the compile command for compiling this target
     and every target that depends on it (via `-I`). An include directory is relative to the current package.
""",
        ),
    }

def _public_system_include_directories_arg():
    return {
        "public_system_include_directories": attrs.set(
            attrs.string(),
            sorted = True,
            default = [],
            doc = """
    A list of include directories (with `raw_headers`) to be added to the compile command for compiling this target
     and every target that depends on it (via `-isystem` if the compiler supports it of via `-I` otherwise).
     An include directory is relative to the current package.
""",
        ),
    }

def _version_arg():
    return {
        "version": attrs.option(
            attrs.string(),
            default = None,
            doc = """
    A string denoting a meaningful version of this rule that is optionally passed to the linker as extra
     metadata.
""",
        ),
    }

def _runtime_dependency_handling_arg():
    return {
        "runtime_dependency_handling": attrs.option(
            attrs.enum(RuntimeDependencyHandling),
            default = None,
            doc = """
    Controls how shared library dependencies are handled at runtime. By default the `no_symlink` behaviour
    is used, which opts out of symlink creation. If `symlink` is specified then shared library dependencies with
    `preferred_linkage = "shared"` are automatically detected and included in a symlink tree
    alongside the executable. Set to `symlink_single_level_only` to only include first-level runtime
    dependencies in a symlink tree.
""",
        ),
    }

def _use_fbcc_rust_wrapper_arg():
    return {
        "use_fbcc_rust_wrapper": attrs.bool(
            default = False, doc = "Opt-in for the rust version of the fbcode C++ Compiler wrapper (replacing the existing fbcc.py)"
        ),
    }

def _use_content_based_paths_arg():
    return {
        # Use content-based paths by default for compile action; targets may opt out
        # explicitly.
        "use_content_based_paths": attrs.bool(default = True),
    }

def _expect_eligible_for_dedupe_arg():
    return {
        "expect_eligible_for_dedupe": attrs.bool(default = False),
    }

def _flags_arg():
    return {
        "flags": attrs.list(
            attrs.dep(providers = [CxxFlagsInfo]),
            default = [],
            doc = """
            `cxx_flags` targets whose flags apply before this target's own. Their transitive
             definitions are combined into a shared argsfile, with nested flags first and
             direct entries in list order. Repeated and diamond dependencies contribute once.
             They cannot contribute `exported_*` or post-linker flag categories.
        """,
        ),
    }

cxx_common = struct(
    srcs_arg = _srcs_arg,
    deps_arg = _deps_arg,
    default_deps_arg = _default_deps_arg,
    supported_platforms_regex_arg = _supported_platforms_regex_arg,
    headers_arg = _headers_arg,
    exported_headers_arg = _exported_headers_arg,
    exported_header_style_arg = _exported_header_style_arg,
    header_namespace_arg = _header_namespace_arg,
    preprocessor_flags_arg = _preprocessor_flags_arg,
    lang_preprocessor_flags_arg = _lang_preprocessor_flags_arg,
    exported_lang_preprocessor_flags_arg = _exported_lang_preprocessor_flags_arg,
    exported_preprocessor_flags_arg = _exported_preprocessor_flags_arg,
    compiler_flags_arg = _compiler_flags_arg,
    lang_compiler_flags_arg = _lang_compiler_flags_arg,
    linker_extra_outputs_arg = _linker_extra_outputs_arg,
    linker_flags_arg = _linker_flags_arg,
    local_linker_flags_arg = _local_linker_flags_arg,
    local_linker_script_flags_arg = _local_linker_script_flags_arg,
    exported_linker_flags_arg = _exported_linker_flags_arg,
    exported_post_linker_flags_arg = _exported_post_linker_flags_arg,
    precompiled_header_arg = _precompiled_header_arg,
    force_static = _force_static,
    reexport_all_header_dependencies_arg = _reexport_all_header_dependencies_arg,
    exported_deps_arg = _exported_deps_arg,
    supports_merged_linking = _supports_merged_linking,
    raw_headers_arg = _raw_headers_arg,
    raw_headers_as_headers_mode_arg = _raw_headers_as_headers_mode_arg,
    include_directories_arg = _include_directories_arg,
    public_include_directories_arg = _public_include_directories_arg,
    public_system_include_directories_arg = _public_system_include_directories_arg,
    version_arg = _version_arg,
    runtime_dependency_handling_arg = _runtime_dependency_handling_arg,
    use_fbcc_rust_wrapper_arg = _use_fbcc_rust_wrapper_arg,
    use_content_based_paths_arg = _use_content_based_paths_arg,
    expect_eligible_for_dedupe_arg = _expect_eligible_for_dedupe_arg,
    flags_arg = _flags_arg,
    supports_stripping = _supports_stripping,
)

BUILD_INFO_ATTR = attrs.dict(
    key = attrs.string(),
    value = attrs.option(attrs.one_of(attrs.string(), attrs.int(), attrs.bool(), attrs.list(attrs.string()))),
    sorted = False,
    default = {},
    doc = "Build info that is passed along here will be late-stamped into a fb_build_info section on the output binary",
)

# Attributes shared by `cxx_binary` and `cxx_test`.
def cxx_binary_and_test_attrs():
    ret = (
        {
            "anonymous_link_groups": attrs.bool(default = False),
            "auto_link_groups": attrs.bool(default = False),
            # Linker flags that only apply to the executable link, used for link
            # strategies (e.g. link groups) which may link shared libraries from
            # top-level binary context.
            "binary_linker_flags": attrs.list(attrs.arg(anon_target_compatible = True), default = []),
            "bolt_flags": attrs.list(attrs.arg(), default = []),
            "bolt_profile": attrs.option(attrs.source(), default = None),
            # These flags will only be used to instrument a target
            # when coverage for that target is enabled by a header
            # selected for coverage either in the target or in one
            # of the target's dependencies.
            "coverage_instrumentation_compiler_flags": attrs.list(attrs.string(), default = []),
            # Optional clang_profile_list target for selective coverage instrumentation via -fprofile-list.
            "coverage_profile_list": attrs.option(attrs.dep(), default = None),
            "cuda_compile_style": attrs.enum(CudaCompileStyle.values(), default = "mono"),
            "enable_distributed_thinlto": attrs.bool(default = False),
            "exported_needs_coverage_instrumentation": attrs.bool(default = False),
            "extra_dwp_flags": attrs.list(attrs.string(), default = []),
            "link_execution_preference": link_execution_preference_attr(),
            "link_group_map": LINK_GROUP_MAP_ATTR,
            "link_group_min_binary_node_count": attrs.option(attrs.int(), default = None),
            "link_ordering": attrs.option(attrs.enum(LinkOrdering.values()), default = None),
            "link_preference": attrs.enum(LinkPreference.values(), default = "default"),
            "link_whole": attrs.default_only(attrs.bool(default = False)),
            "precompiled_header": attrs.option(attrs.dep(providers = [CPrecompiledHeaderInfo]), default = None),
            "resources": attrs.named_set(attrs.one_of(attrs.dep(), attrs.source(allow_directory = True)), sorted = True, default = []),
            "separate_debug_info": attrs.bool(default = False),
            "_build_info": BUILD_INFO_ATTR,
            "_cxx_hacks": attrs.dep(default = "prelude//cxx/tools:cxx_hacks"),
            "_cxx_toolchain": toolchains_common.cxx(),
        }
        | cxx_common.flags_arg()
        | validation_common.attrs_validators_arg()
    )
    ret.update(constraint_overrides.attributes)
    return ret

# Attributes shared by `cxx_library` and `cxx_precompiled_header`.
_library_attrs = (
    # @unsorted-dict-items
    cxx_common.srcs_arg()
    | cxx_common.flags_arg()
    | cxx_common.headers_arg()
    | cxx_common.exported_headers_arg()
    | cxx_common.exported_header_style_arg()
    | cxx_common.header_namespace_arg()
    | cxx_common.preprocessor_flags_arg()
    | cxx_common.lang_preprocessor_flags_arg()
    | cxx_common.exported_preprocessor_flags_arg(exported_preprocessor_flags_type = attrs.list(attrs.arg(), default = []))
    | cxx_common.exported_lang_preprocessor_flags_arg()
    | cxx_common.compiler_flags_arg()
    | cxx_common.lang_compiler_flags_arg()
    | cxx_common.linker_extra_outputs_arg()
    | cxx_common.linker_flags_arg()
    | cxx_common.local_linker_flags_arg()
    | cxx_common.exported_linker_flags_arg()
    | cxx_common.exported_post_linker_flags_arg()
    | native_common.link_style()
    | native_common.link_whole(link_whole_type = attrs.option(attrs.bool(), default = None))
    | native_common.soname()
    | cxx_common.raw_headers_arg()
    | cxx_common.supports_stripping()
    | cxx_common.raw_headers_as_headers_mode_arg()
    | cxx_common.include_directories_arg()
    | cxx_common.public_include_directories_arg()
    | cxx_common.public_system_include_directories_arg()
    | {
        "deffile": attrs.option(
            attrs.source(),
            default = None,
            doc = """
            Specifies the *.def file used on windows to modify a dll's exports in place of explicit `__declspec(dllexport)` declarations.
              The default is to not use a defile.
        """,
        ),
        "used_by_wrap_script": attrs.bool(
            default = False,
            doc = """
            When using an exopackage
              Android, if this parameter is set to `True`, then the library is
              included in the primary APK even if native libraries would otherwise not be
              placed in it. This is intended for native libraries that are used by a
              [wrap.sh](https://developer.android.com/ndk/guides/wrap-script)
              script, which must be placed in the primary APK. Only one of
              `can_be_asset` and `used_by_wrap_script` can be set
              for a rule.
        """,
        ),
    }
    | cxx_common.supported_platforms_regex_arg()
    | cxx_common.force_static(force_static_type = attrs.option(attrs.bool(), default = None))
    | native_common.preferred_linkage(preferred_linkage_type = attrs.option(attrs.enum(Linkage.values()), default = None))
    | cxx_common.reexport_all_header_dependencies_arg()
    | cxx_common.exported_deps_arg()
    | cxx_common.precompiled_header_arg()
    | apple_common.extra_xcode_sources()
    | apple_common.extra_xcode_files()
    | apple_common.uses_explicit_modules_arg()
    | apple_common.meta_apple_library_validation_enabled_arg()
    | cxx_common.version_arg()
    | cxx_common.use_fbcc_rust_wrapper_arg()
    | cxx_common.use_content_based_paths_arg()
    | cxx_common.expect_eligible_for_dedupe_arg()
    | validation_common.attrs_validators_arg()
    | {
        "archive_allow_cache_upload": attrs.bool(default = False),
        "archive_contents": attrs.option(attrs.enum(ArchiveContentsType.values()), default = None),
        "bridging_header": attrs.option(attrs.source(), default = None),
        "can_be_asset": attrs.option(attrs.bool(), default = None),
        "cxx_runtime_type": attrs.option(attrs.enum(CxxRuntimeType), default = None),
        "default_host_platform": attrs.option(attrs.configuration_label(), default = None),
        "default_platform": attrs.option(attrs.string(), default = None),
        "defaults": attrs.dict(key = attrs.string(), value = attrs.string(), sorted = False, default = {}),
        "deps": attrs.list(attrs.dep(), default = []),
        "devirt_enabled": attrs.bool(default = False),
        "diagnostics": attrs.dict(key = attrs.string(), value = attrs.source(), sorted = False, default = {}),
        "executable_name": attrs.option(attrs.string(), default = None),
        "export_header_unit": attrs.option(
            attrs.enum(["include", "preload"]),
            default = None,
            doc = """
            If not None, export a C++20 header unit visible to dependants (including
            recursively) with use_header_units set to True.

            "include": replace includes of each file in exported_headers or
                raw_headers with an import of the precompiled header unit; files
                that do not include any of those headers do not load the header
                unit.

            "preload": automatically load the precompiled header unit in any
                dependant that uses header units.
        """,
        ),
        "export_header_unit_filter": attrs.list(
            attrs.string(),
            default = [],
            doc = """
            A list of regexes. Each regex should match a set of headers in
            exported_headers or raw_headers to be precompiled together into one
            C++20 header unit.

            When used with export_header_unit="include", this allows different
            subsets of headers to be loaded only by files that use them. Each group
            should only depend on headers in previous groups.

            If a header is not matched by any group, it is not precompiled and will
            be included textually. If no filter is specified, the rule excludes
            inline headers based on a name heuristics (e.g. "-inl.h").
        """,
        ),
        "extra_dwp_flags": attrs.list(attrs.string(), default = []),
        "fat_lto": attrs.bool(default = False),
        "focused_list_target": attrs.option(attrs.dep(), default = None),
        "frameworks": attrs.list(attrs.string(), default = []),
        "headers_as_raw_headers_mode": attrs.option(attrs.enum(HeadersAsRawHeadersMode), default = None),
        "include_in_android_merge_map_output": attrs.bool(default = True),
        "libraries": attrs.list(attrs.string(), default = []),
        "link_group": attrs.option(attrs.string(), default = None),
        "link_group_map": LINK_GROUP_MAP_ATTR,
        "module_name": attrs.option(attrs.string(), default = None),
        "post_linker_flags": attrs.list(attrs.arg(anon_target_compatible = True), default = []),
        "prefix_header": attrs.option(attrs.source(), default = None),
        "resources": attrs.named_set(attrs.source(), sorted = True, default = []),
        "sdk_modules": attrs.list(attrs.string(), default = []),
        "static_library_basename": attrs.option(attrs.string(), default = None),
        "supports_merged_linking": attrs.option(attrs.bool(), default = None),
        "thin_lto": attrs.bool(default = False),
        "use_archive": attrs.option(attrs.bool(), default = None),
        "use_header_units": attrs.one_of(
            attrs.bool(),
            attrs.enum(["stub"]),
            default = False,
            doc = """
            If True, makes any header unit exported by a dependency (including
            recursively) through export_header_unit available to the compiler. If
            false, the compilation ignores header units, regardless of what is
            exported by dependencies. If "stub", uses stub header units instead
            of full PCM files.
        """,
        ),
        "uses_cxx_explicit_modules": attrs.bool(default = False),
        "version_universe": attrs.option(attrs.string(), default = None),
        "weak_framework_names": attrs.list(attrs.string(), default = []),
    }
    | buck.allow_cache_upload_arg()
    | buck.licenses_arg()
    | buck.labels_arg()
    | buck.contacts_arg()
)

# The attributes `cxx_library` had in Buck1. `cxx_python_extension` and the
# Cython rules copy these as default-only attributes; `cxx_library` itself has
# `cxx_library_extra_attrs()` on top.
def cxx_library_base_attrs() -> dict[str, Attr]:
    return _library_attrs | cxx_common.default_deps_arg("exported_deps")

def cxx_library_extra_attrs() -> dict[str, Attr]:
    return (
        {
            "auto_link_groups": attrs.bool(default = False),
            # These flags will only be used to instrument a target
            # when coverage for that target is enabled by `exported_needs_coverage_instrumentation`
            # or by any of the target's dependencies.
            "coverage_instrumentation_compiler_flags": attrs.list(attrs.string(), default = []),
            # Optional clang_profile_list target for selective coverage instrumentation via -fprofile-list.
            "coverage_profile_list": attrs.option(attrs.dep(), default = None),
            "cuda_compile_style": attrs.enum(CudaCompileStyle.values(), default = "mono"),
            "deps_query": attrs.option(attrs.query(), default = None),
            "exported_needs_coverage_instrumentation": attrs.bool(default = False),
            "extra_dwp_flags": attrs.list(attrs.string(), default = []),
            "header_mode": attrs.option(attrs.enum(HeaderMode.values()), default = None),
            "link_deps_query_whole": attrs.bool(default = False),
            "link_execution_preference": link_execution_preference_attr(),
            "link_group_map": LINK_GROUP_MAP_ATTR,
            "link_ordering": attrs.option(attrs.enum(LinkOrdering.values()), default = None),
            "precompiled_header": attrs.option(attrs.dep(providers = [CPrecompiledHeaderInfo]), default = None),
            "prefer_stripped_objects": attrs.bool(default = False),
            "preferred_linkage": attrs.enum(
                Linkage.values(),
                default = "any",
                doc = """
    Determines what linkage is used when the library is depended on by another target. To
    control how the dependencies of this library are linked, use `link_style` instead.
    """,
            ),
            "resources": attrs.named_set(attrs.one_of(attrs.dep(), attrs.source(allow_directory = True)), sorted = True, default = []),
            "separate_debug_info": attrs.bool(default = False),
            "stub": attrs.bool(default = False),
            "supports_header_symlink_subtarget": attrs.bool(default = False),
            "supports_python_dlopen": attrs.option(attrs.bool(), default = None),
            "supports_shlib_interfaces": attrs.bool(default = True),
            "third_party_project": attrs.option(attrs.string(), default = None),
            "_cxx_hacks": attrs.default_only(attrs.dep(default = "prelude//cxx/tools:cxx_hacks")),
            "_cxx_toolchain": toolchains_common.cxx(),
            "_emit_native_build_commands": attrs.default_only(attrs.bool(default = EMIT_NATIVE_BUILD_COMMANDS)),
            "_is_building_android_binary": is_building_android_binary_attr(),
        }
        | apple_common.extra_xcode_sources()
        | third_party_common.create_third_party_build_root_attrs()
    )

# The attributes `cxx_binary` had in Buck1. `python_binary` and `python_test`
# copy these as default-only attributes.
def cxx_binary_base_attrs() -> dict[str, Attr]:
    return (
        # @unsorted-dict-items
        cxx_common.srcs_arg()
        | cxx_common.headers_arg()
        | cxx_common.header_namespace_arg()
        | cxx_common.preprocessor_flags_arg()
        | cxx_common.compiler_flags_arg()
        | cxx_common.linker_extra_outputs_arg()
        | cxx_common.linker_flags_arg()
        | cxx_common.precompiled_header_arg()
        | cxx_common.default_deps_arg()
        | native_common.link_style()
        | native_common.link_group_deps()
        | native_common.link_group_public_deps_label()
        | native_common.transformation_spec_arg()
        | buck.deps_query_arg()
        | cxx_common.raw_headers_arg()
        | cxx_common.include_directories_arg()
        | cxx_common.raw_headers_as_headers_mode_arg()
        | cxx_common.runtime_dependency_handling_arg()
        | cxx_common.use_fbcc_rust_wrapper_arg()
        | cxx_common.use_content_based_paths_arg()
        | {
            "cxx_runtime_type": attrs.option(attrs.enum(CxxRuntimeType), default = None),
            "default_platform": attrs.option(attrs.string(), default = None),
            "defaults": attrs.dict(key = attrs.string(), value = attrs.string(), sorted = False, default = {}),
            "deps": attrs.list(attrs.dep(), default = []),
            "devirt_enabled": attrs.bool(default = False),
            "executable_name": attrs.option(attrs.string(), default = None),
            "fat_lto": attrs.bool(default = False),
            "focused_list_target": attrs.option(attrs.dep(), default = None),
            "frameworks": attrs.list(attrs.string(), default = []),
            "headers_as_raw_headers_mode": attrs.option(attrs.enum(HeadersAsRawHeadersMode), default = None),
            "lang_compiler_flags": attrs.dict(key = attrs.enum(CxxSourceType), value = attrs.list(attrs.arg()), sorted = False, default = {}),
            "lang_preprocessor_flags": attrs.dict(key = attrs.enum(CxxSourceType), value = attrs.list(attrs.arg()), sorted = False, default = {}),
            "libraries": attrs.list(attrs.string(), default = []),
            "link_deps_query_whole": attrs.bool(default = False),
            "link_group": attrs.option(attrs.string(), default = None),
            "link_group_map": LINK_GROUP_MAP_ATTR,
            "post_linker_flags": attrs.list(attrs.arg(anon_target_compatible = True), default = []),
            "prefer_stripped_objects": attrs.bool(default = False),
            "prefix_header": attrs.option(attrs.source(), default = None),
            "resources": attrs.named_set(attrs.source(), sorted = True, default = []),
            "thin_lto": attrs.bool(default = False),
            "use_header_units": attrs.one_of(attrs.bool(), attrs.enum(["stub"]), default = False),
            "version_universe": attrs.option(attrs.string(), default = None),
            "weak_framework_names": attrs.list(attrs.string(), default = []),
        }
        | buck.allow_cache_upload_arg()
        | buck.licenses_arg()
        | buck.labels_arg()
        | buck.contacts_arg()
        | cxx_binary_and_test_attrs()
    )

def cxx_precompiled_header_attrs() -> dict[str, Attr]:
    return (
        cxx_common.use_fbcc_rust_wrapper_arg()
        | {
            "compile_pch_file": attrs.bool(
                default = False,
                doc = """
                Whether to compile the precompiled header file or use legacy mode.
            """,
            ),
            "deps": attrs.list(
                attrs.dep(),
                default = [],
                doc = """
                Dependency rules which export headers used by the header specified in `src`.
            """,
            ),
            "pch_clanguage": attrs.option(
                attrs.string(),
                default = None,
                doc = """
                The c-language extension to use for the precompiled header. Eg. .c, .cpp, .m, .mm, etc.
            """,
            ),
            "src": attrs.source(
                doc = """
                The path to the header file that should be precompiled.
                 Only one header file can be specified. But of course this header could include
                 any number of other headers. The included headers could belong to -- that is,
                 be `exported_headers` from -- another rule, in which case, the rule would
                 have to be added to `deps` as usual.
            """
            ),
            "version_universe": attrs.option(attrs.string(), default = None),
        }
        | _library_attrs
        | buck.licenses_arg()
        | buck.labels_arg()
        | buck.contacts_arg()
    )
