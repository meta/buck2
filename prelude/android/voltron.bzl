# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

load("@prelude//:prelude_rule.bzl", "prelude_rule")
load("@prelude//android:android_providers.bzl", "AndroidPackageableInfo", "merge_android_packageable_info")
load("@prelude//android:android_toolchain.bzl", "AndroidToolchainInfo")
load("@prelude//android:build_only_native_code.bzl", "is_build_only_native_code")
load("@prelude//decls:common.bzl", "buck")
load("@prelude//decls:toolchains_common.bzl", "toolchains_common")
load("@prelude//java:java_providers.bzl", "get_all_java_packaging_deps")
load(
    "@prelude//linking:shared_libraries.bzl",
    "SharedLibrary",  # @unused Used as type
    "SharedLibraryInfo",
    "merge_shared_libraries",
    "traverse_shared_library_info",
)
load("@prelude//utils:argfile.bzl", "argfile")
load("@prelude//utils:expect.bzl", "expect")
load("@prelude//utils:utils.bzl", "flatten")

# "Voltron" gives us the ability to split our Android APKs into different "modules". These
# modules can then be downloaded on demand rather than shipped with the "main" APK.
#
# The module corresponding to the "main" APK is called the "root" module.
#
# Voltron support comes in two main parts:
# (1) Constructing the Voltron module graph (assigning targets to each module). This is done
# by constructing a "target graph" and then delegating to buck1 to produce the module graph.
# (2) Using the Voltron module graph while building our APK.
#
# For (1), in order to calculate which targets belong to each module, we reconstruct a "target
# graph" from "deps" information that is propagated up through AndroidPackageableInfo.
# In buck1 we use the underlying "TargetGraph" object that is based on the raw target
# definitions. This results in some slightly different behavior for `provided_deps` - in
# buck2, we (correctly) ignore `provided_deps`, since they do not influence the packaging of
# the APK, whereas in `buck1`, we treat `provided_deps` the same as `deps`.
# In practice, this rarely affects the module assignments, but can mean that `buck2` will
# put a target inside a module whereas `buck1` will put it into the main APK (since `buck1`
# can find a path from an "always in main APK seed" to the target via some `provided_dep`,
# whereas `buck2` does not).
#
# For (2), we package up secondary DEX files and native libs into `assets/module_name` (see
# dex_rules.bzl and android_binary_native_rules.bzl for more information on how we do that).
# It is worth noting that we still put all of the non-root modules into the final APK. If
# the module should be downloaded on demand, then it is removed from the final APK in a
# subsequent post-processing step.
#
# There is also an `android_app_modularity` rule that just prints out details of the Voltron
# module graph and is used for any subsequent verification.
def android_app_modularity_impl(ctx: AnalysisContext) -> list[Provider]:
    if ctx.attrs._build_only_native_code:
        return [
            # Add an unused default output in case this target is used as an attr.source() anywhere.
            DefaultInfo(default_output = ctx.actions.write("{}/unused.txt".format(ctx.label.name), [], has_content_based_path = False)),
        ]

    all_deps = ctx.attrs.deps + flatten(ctx.attrs.application_module_configs.values())
    android_packageable_info = merge_android_packageable_info(ctx.label, ctx.actions, all_deps)

    shared_library_info = merge_shared_libraries(
        ctx.actions,
        deps = filter(None, [x.get(SharedLibraryInfo) for x in all_deps]),
    )
    traversed_shared_library_info = traverse_shared_library_info(shared_library_info, transformation_provider = None)

    cmd, output = _get_base_cmd_and_output(
        ctx.actions,
        ctx.label,
        [android_packageable_info],
        traversed_shared_library_info,
        ctx.attrs._android_toolchain[AndroidToolchainInfo],
        ctx.attrs.application_module_configs,
        ctx.attrs.application_module_dependencies,
        ctx.attrs.application_module_blocklist,
    )

    if ctx.attrs.should_include_classes:
        no_dx_target_labels = [no_dx_target.label.raw_target() for no_dx_target in ctx.attrs.no_dx]
        java_packaging_deps = [
            packaging_dep
            for packaging_dep in get_all_java_packaging_deps(ctx, all_deps)
            if packaging_dep.dex and packaging_dep.dex.dex.owner.raw_target() not in no_dx_target_labels
        ]
        targets_to_jars_args = [cmd_args([str(packaging_dep.label.raw_target()), packaging_dep.jar], delimiter = " ") for packaging_dep in java_packaging_deps]
        targets_to_jars = argfile(actions = ctx.actions, name = "targets_to_jars.txt", args = targets_to_jars_args)
        cmd.add([
            "--targets-to-jars",
            targets_to_jars,
        ])

    if ctx.attrs.should_include_libraries:
        targets_to_so_names_args = [
            cmd_args([str(shared_lib.label.raw_target()), shared_lib.soname.ensure_str()], delimiter = " ") for shared_lib in traversed_shared_library_info
        ]
        targets_to_so_names = argfile(actions = ctx.actions, name = "targets_to_so_names.txt", args = targets_to_so_names_args)
        cmd.add([
            "--targets-to-so-names",
            targets_to_so_names,
        ])

        traversed_prebuilt_native_library_dirs = (
            android_packageable_info.prebuilt_native_library_dirs.traverse() if android_packageable_info.prebuilt_native_library_dirs else []
        )
        targets_to_non_assets_prebuilt_native_library_dirs_args = [
            cmd_args(prebuilt_native_library_dir.raw_target, prebuilt_native_library_dir.dir, delimiter = " ")
            for prebuilt_native_library_dir in traversed_prebuilt_native_library_dirs
            if not prebuilt_native_library_dir.is_asset and not prebuilt_native_library_dir.for_primary_apk
        ]
        targets_to_non_assets_prebuilt_native_library_dirs = argfile(
            actions = ctx.actions,
            name = "targets_to_non_assets_prebuilt_native_library_dirs.txt",
            args = targets_to_non_assets_prebuilt_native_library_dirs_args,
        )
        cmd.add([
            "--targets-to-non-asset-prebuilt-native-library-dirs",
            targets_to_non_assets_prebuilt_native_library_dirs,
        ])

    ctx.actions.run(cmd, category = "apk_module_graph")

    return [DefaultInfo(default_output = output)]

def get_target_to_module_mapping(ctx: AnalysisContext, deps_by_platform: dict[str, list[Dependency]]) -> Artifact | None:
    if not ctx.attrs.application_module_configs:
        return None

    android_packageable_infos = []
    shared_libraries = []
    for deps in deps_by_platform.values() + [flatten(ctx.attrs.application_module_configs.values())]:
        android_packageable_infos.append(merge_android_packageable_info(ctx.label, ctx.actions, deps))
        shared_library_info = merge_shared_libraries(
            ctx.actions,
            deps = filter(None, [x.get(SharedLibraryInfo) for x in deps]),
        )
        shared_libraries.extend(traverse_shared_library_info(shared_library_info, transformation_provider = None))

    cmd, output = _get_base_cmd_and_output(
        ctx.actions,
        ctx.label,
        android_packageable_infos,
        shared_libraries,
        ctx.attrs._android_toolchain[AndroidToolchainInfo],
        ctx.attrs.application_module_configs,
        ctx.attrs.application_module_dependencies,
        ctx.attrs.application_module_blocklist,
    )

    cmd.add("--output-module-info-and-target-to-module-only")

    ctx.actions.run(cmd, category = "apk_module_graph")

    return output

def _get_base_cmd_and_output(
    actions: AnalysisActions,
    label: Label,
    android_packageable_infos: list[AndroidPackageableInfo],
    shared_libraries: list[SharedLibrary],
    android_toolchain: AndroidToolchainInfo,
    application_module_configs: dict[str, list[Dependency]],
    application_module_dependencies: [dict[str, list[str]], None],
    application_module_blocklist: [list[Dependency], None],
) -> (cmd_args, Artifact):
    deps_map = {}
    primary_apk_deps = set()
    for android_packageable_info in android_packageable_infos:
        if android_packageable_info.deps:
            for deps_info in android_packageable_info.deps.traverse():
                deps = deps_map.setdefault(deps_info.name, [])
                deps_map[deps_info.name] = dedupe(deps + deps_info.deps)
                if deps_info.for_primary_apk:
                    primary_apk_deps.add(deps_info.name)

    target_graph_file = actions.write_json("target_graph.json", deps_map, has_content_based_path = False)
    application_module_configs_map = {
        module_name: [seed.get(AndroidPackageableInfo).target_label for seed in seeds if seed.get(AndroidPackageableInfo)]
        for module_name, seeds in application_module_configs.items()
    }
    application_module_configs_file = actions.write_json("application_module_configs.json", application_module_configs_map, has_content_based_path = False)
    application_module_dependencies_file = actions.write_json(
        "application_module_dependencies.json", application_module_dependencies or {}, has_content_based_path = False
    )
    output = actions.declare_output("apk_module_metadata.txt", has_content_based_path = False)

    cmd = cmd_args([
        android_toolchain.apk_module_graph[RunInfo],
        "--root-target",
        str(label.raw_target()),
        "--target-graph",
        target_graph_file,
        "--seed-config-map",
        application_module_configs_file,
        "--app-module-dependencies-map",
        application_module_dependencies_file,
        "--output",
        output.as_output(),
    ])

    # Anything that is used by a wrap script needs to go into the primary APK, as do all
    # of their deps.
    used_by_wrap_script_libs = [str(shared_lib.label.raw_target()) for shared_lib in shared_libraries if shared_lib.for_primary_apk]
    prebuilt_native_library_dirs = flatten([
        list(android_packageable_info.prebuilt_native_library_dirs.traverse()) if android_packageable_info.prebuilt_native_library_dirs else []
        for android_packageable_info in android_packageable_infos
    ])
    prebuilt_native_library_targets_for_primary_apk = dedupe(
        [str(native_lib_dir.raw_target) for native_lib_dir in prebuilt_native_library_dirs if native_lib_dir.for_primary_apk]
    )
    if application_module_blocklist or used_by_wrap_script_libs or prebuilt_native_library_targets_for_primary_apk or len(primary_apk_deps) > 0:
        all_blocklisted_deps = used_by_wrap_script_libs + prebuilt_native_library_targets_for_primary_apk + list(primary_apk_deps)
        if application_module_blocklist:
            all_blocklisted_deps.extend([str(blocklisted_dep.label.raw_target()) for blocklisted_dep in application_module_blocklist])

        application_module_blocklist_file = actions.write(
            "application_module_blocklist.txt",
            all_blocklisted_deps,
            has_content_based_path = False,
        )
        cmd.add([
            "--always-in-main-apk-seeds",
            application_module_blocklist_file,
        ])

    return cmd, output

ROOT_MODULE = "dex"

def is_root_module(module: str) -> bool:
    return module == ROOT_MODULE

def all_targets_in_root_module(_module: str) -> str:
    return ROOT_MODULE

APKModuleGraphInfo = record(
    module_list = list[str],
    target_to_module_mapping_function = typing.Callable,
    module_to_canary_class_name_function = typing.Callable,
    module_to_module_deps_function = typing.Callable,
    transitive_module_deps_function = typing.Callable,
    calculated_deps_function = typing.Callable,
    get_deps_debug_data = typing.Callable,
)

def get_root_module_only_apk_module_graph_info() -> APKModuleGraphInfo:
    def root_module_canary_class_name(module: str):
        expect(is_root_module(module))
        return "secondary"

    def root_module_deps(module: str):
        expect(is_root_module(module))
        return []

    return APKModuleGraphInfo(
        module_list = [ROOT_MODULE],
        target_to_module_mapping_function = all_targets_in_root_module,
        module_to_canary_class_name_function = root_module_canary_class_name,
        module_to_module_deps_function = root_module_deps,
        transitive_module_deps_function = root_module_deps,
        calculated_deps_function = root_module_deps,
        get_deps_debug_data = root_module_deps,
    )

def get_apk_module_graph_info(ctx: AnalysisContext, apk_module_graph_file: Artifact, artifacts) -> APKModuleGraphInfo:
    apk_module_graph_lines = artifacts[apk_module_graph_file].read_string().split("\n")
    module_count = int(apk_module_graph_lines[0])
    module_infos = apk_module_graph_lines[1 : module_count + 1]
    target_to_module_lines = apk_module_graph_lines[module_count + 1 : -1]
    expect(apk_module_graph_lines[-1] == "", "Expect last line to be an empty string!")

    module_to_canary_class_name_map = {}
    module_to_module_deps_map = {}
    transitive_module_deps_map = {}
    calculated_deps_map = {}
    shared_module_rdeps = {}
    for line in module_infos:
        line_data = line.split(" ")
        module_name = line_data[0]
        canary_class_name = line_data[1]
        module_deps = [module_dep for module_dep in line_data[2:] if module_dep]
        module_to_canary_class_name_map[module_name] = canary_class_name
        module_to_module_deps_map[module_name] = module_deps
        shared_modules = [module_dep for module_dep in module_deps if module_dep.startswith("s_")]
        for shared_module in shared_modules:
            rdeps = shared_module_rdeps.get(shared_module, set())
            rdeps.add(module_name)
            shared_module_rdeps[shared_module] = rdeps

    target_to_module_mapping = {str(ctx.label.raw_target()): ROOT_MODULE}
    for line in target_to_module_lines:
        target, module = line.split(" ")
        target_to_module_mapping[target] = module

    for module, deps in module_to_module_deps_map.items():
        visited_modules = set()
        queue = [d for d in deps]
        for _ in range(1, 1000):  # represents a while loop since while loops dont exist in starlark
            if len(queue) == 0:
                transitive_module_deps_map[module] = visited_modules
                continue
            node = queue.pop()
            visited_modules.add(node)
            for d in module_to_module_deps_map[node]:
                if d not in visited_modules:
                    queue.append(d)
    for shared_module, rdeps in shared_module_rdeps.items():
        rdeps_list = list(rdeps)
        head = rdeps_list[0]
        intersection = transitive_module_deps_map[head]
        for rdep in rdeps_list[1:]:
            intersection = intersection & transitive_module_deps_map[rdep]
        calculated_deps_map[shared_module] = intersection | rdeps

    def target_to_module_mapping_function(raw_target: str) -> str:
        mapped_module = target_to_module_mapping.get(raw_target)
        expect(mapped_module != None, "No module found for target {}!".format(raw_target))
        return mapped_module

    def module_to_canary_class_name_function(voltron_module: str) -> str:
        return module_to_canary_class_name_map.get(voltron_module)

    def module_to_module_deps_function(voltron_module: str) -> list:
        return module_to_module_deps_map.get(voltron_module)

    def transitive_module_deps_function(voltron_module: str) -> set[str]:
        return transitive_module_deps_map.get(voltron_module)

    def calculated_deps_function(voltron_module: str) -> set[str]:
        return calculated_deps_map.get(voltron_module) if voltron_module in calculated_deps_map else set()

    def get_deps_debug_data() -> str:
        return "tdeps - {} \n calculated deps - {}".format(transitive_module_deps_map, calculated_deps_map)

    return APKModuleGraphInfo(
        module_list = module_to_canary_class_name_map.keys(),
        target_to_module_mapping_function = target_to_module_mapping_function,
        module_to_canary_class_name_function = module_to_canary_class_name_function,
        module_to_module_deps_function = module_to_module_deps_function,
        transitive_module_deps_function = transitive_module_deps_function,
        calculated_deps_function = calculated_deps_function,
        get_deps_debug_data = get_deps_debug_data,
    )

android_app_modularity = prelude_rule(
    name = "android_app_modularity",
    impl = android_app_modularity_impl,
    docs = """
        An `android_app_modularity()` rule is used to determine the modular
        structure of an Android app. It defines how application modules are
        configured and which dependencies belong to each module, enabling
        dynamic delivery and on-demand loading of app features.
    """,
    examples = None,
    further = None,
    attrs = (
        # @unsorted-dict-items
        {
            "application_module_blocklist": attrs.option(attrs.list(attrs.dep()), default = None, doc = ""),
            "application_module_configs": attrs.dict(key = attrs.string(), value = attrs.list(attrs.dep()), sorted = False, default = {}, doc = ""),
            "application_module_dependencies": attrs.option(
                attrs.dict(key = attrs.string(), value = attrs.list(attrs.string()), sorted = False),
                default = None,
                doc = "",
            ),
            "deps": attrs.list(attrs.dep(), default = [], doc = ""),
            "no_dx": attrs.list(
                attrs.dep(),
                default = [],
                doc = """List of dependency targets whose Java code should be excluded from the DEX files in the final APK. \
The classes from these targets will not be dexed, but their other resources will still be included. \
This might be useful to resolve duplicate class errors when multiple dependencies provide the same class. \
See [this document](https://www.internalfb.com/intern/staticdocs/confucius/docs/notes/dev/projects/android/resolving_duplicate_classes_with_no_dx/) \
for how to resolve duplicate classes using this attribute.""",
            ),
            "should_include_classes": attrs.bool(default = True, doc = ""),
            "should_include_libraries": attrs.bool(default = False, doc = ""),
            "_android_toolchain": toolchains_common.android(),
            "_build_only_native_code": attrs.default_only(attrs.bool(default = is_build_only_native_code())),
        }
        | buck.licenses_arg()
        | buck.labels_arg()
        | buck.contacts_arg()
    ),
)
