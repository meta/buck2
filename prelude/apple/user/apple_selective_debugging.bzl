# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

load(
    "@prelude//:artifact_tset.bzl",
    "ArtifactInfo",
    "ArtifactInfoTag",
    "ArtifactTSet",
)
load("@prelude//apple:apple_toolchain_types.bzl", "AppleToolchainInfo", "AppleToolsInfo")
load(
    "@prelude//linking:execution_preference.bzl",
    "LinkExecutionPreference",
    "LinkExecutionPreferenceDeterminatorInfo",
    "LinkExecutionPreferenceInfo",  # @unused Used as a type
    "get_action_execution_attributes",
)
load(
    "@prelude//utils:build_target_pattern.bzl",
    "BuildTargetPattern",  # @unused Used as a type
    "parse_build_target_pattern",
)
load("@prelude//utils:lazy.bzl", "lazy")

_SelectionCriteria = record(
    include_build_target_patterns = field(list[BuildTargetPattern], []),
    include_regular_expressions = field(list[regex], []),
    exclude_build_target_patterns = field(list[BuildTargetPattern], []),
    exclude_regular_expressions = field(list[regex], []),
)

AppleSelectiveDebuggingInfo = provider(
    # @unsorted-dict-items
    fields = {
        "scrub_binary": provider_field(typing.Callable),
        "filter": provider_field(typing.Callable),
        "scrub_selected_debug_paths_file": provider_field(typing.Callable),
        # Set when using the `dsym_allowlist` filtering strategy, `None` otherwise.
        "generate_dsym_allowlist": provider_field(typing.Callable | None, default = None),
        # Set when the selected targets are only known at build time (`targets`
        # json type), `None` otherwise. Takes the context, an `ArtifactTSet` and
        # an output name, returns an `AppleSelectedDebugInfo` with the artifacts
        # of the selected targets, using the same selection as `filter`. With a
        # `dsym_binary` (and its link execution preference), returns the inputs
        # its dSYM needs instead: the artifacts of the selected targets linked into
        # it, and all `.swiftmodule` files.
        # Each node of the `ArtifactTSet` must hold the debug info of a whole
        # binary, in topological order, like the result of `filter` or an
        # aggregate of such results: the Swift debug info of the dependencies of a
        # selected target is only kept from the node that holds the target.
        "select_debug_info": provider_field(typing.Callable | None, default = None),
    },
)

# The artifacts of the selected targets, picked at build time.
AppleSelectedDebugInfo = record(
    # Directory of symlinks to the selected artifacts. Using it as an action input,
    # or materializing it, makes the selected artifacts available at their own paths.
    artifacts = field(Artifact),
    # The paths of the selected artifacts, one per line. It is a file inside
    # `artifacts`, so materializing it also materializes them. An action needs
    # `artifacts` itself as an input to get them.
    paths = field(Artifact),
)

_SELECTED_PATHS_FILE_NAME = "debuginfo.artifacts"

AppleSelectiveDebuggingFilteredDebugInfo = record(
    # Contains all artifacts, including those required for debugging (e.g., `.swiftmodule` files)
    infos = field(list[ArtifactInfo]),
    # Contains only artifacts for _selected_ targets, excluding any others
    selected_target_infos = field(list[ArtifactInfo]),
    swift_modules_labels = field(list[Label]),
    metadata = field(Artifact),
)

# The type of selective debugging json input to utilze.
SelectiveDebuggingJsonTypes = [
    # Use a targets json file containing all targets to include.
    "targets",
    # Use a spec json file specifying the targets to include
    # and exclude via build target patterns and regular expressions.
    "spec",
]

_SelectiveDebuggingJsonType = enum(*SelectiveDebuggingJsonTypes)

# How the debug info of non-selected targets is excluded.
SelectiveDebuggingFilteringStrategies = [
    # Rewrite the debug map (`N_OSO` entries) of linked binaries, so that the
    # debug info of non-selected targets cannot be found.
    "scrub_binary",
    # Leave linked binaries untouched and only restrict which objects get
    # linked into dSYMs, using `dsymutil --allow`. Requires a `dsymutil`
    # which supports that option.
    "dsym_allowlist",
]

_SelectiveDebuggingFilteringStrategy = enum(*SelectiveDebuggingFilteringStrategies)

_LOCAL_LINK_THRESHOLD = 0.2

_OBJECT_FILE_EXTENSIONS = [
    ".o",
    ".a",
]

def _apple_skip_adhoc_resigning_scrubbed_frameworks_attr_value(bundle_ctx: AnalysisContext) -> bool:
    override_value = bundle_ctx.attrs._skip_adhoc_resigning_scrubbed_frameworks_override
    if override_value != None:
        # Override takes precedence over any other value
        return override_value

    skip = bundle_ctx.attrs.skip_adhoc_resigning_scrubbed_frameworks
    return skip if skip != None else bundle_ctx.attrs._skip_adhoc_resigning_scrubbed_frameworks_default

def _should_skip_adhoc_sign_after_scrubbing(bundle_ctx: AnalysisContext) -> bool:
    sdk_name = bundle_ctx.attrs._apple_toolchain[AppleToolchainInfo].sdk_name
    if bundle_ctx.attrs.extension == "framework" and "iphonesimulator" in sdk_name:
        # While scrubbing invalidates the code signature, the signature of frameworks
        # are generally not checked by the system (unless running against the hardened
        # runtime for macOS apps). Re-signing scrubbed frameworks can take non-trivial
        # time, so skip it if it's not needed.
        return _apple_skip_adhoc_resigning_scrubbed_frameworks_attr_value(bundle_ctx)
    return False

def _generate_metadata_json_object(is_any_selected_target_linked: bool) -> dict[str, typing.Any]:
    return {
        "contains_focused_targets": is_any_selected_target_linked,
    }

# Keeps the artifacts of the selected targets and, once a selected target contains
# Swift, the Swift debug info of the targets which follow it (i.e. its
# dependencies, in topological order). This is the selection `filter_debug_info()`
# makes at analysis time.
def _select_debug_infos(infos: list[ArtifactInfo], is_label_selected: typing.Callable[[Label], bool]) -> list[ArtifactInfo]:
    selected = []
    selected_targets_contain_swift = False
    for info in infos:
        is_swift_debug_info = ArtifactInfoTag("swift_debug_info") in info.tags
        if is_label_selected(info.label) or (selected_targets_contain_swift and is_swift_debug_info):
            selected.append(info)
            selected_targets_contain_swift = selected_targets_contain_swift or is_swift_debug_info
    return selected

def _select_debug_info_impl(
    actions: AnalysisActions,
    debug_info_tset: ArtifactTSet,
    linked_targets_json: ArtifactValue | None,
    targets_json: ArtifactValue,
    artifacts: OutputArtifact,
    paths: OutputArtifact,
) -> list[Provider]:
    # Like the scrubber, match targets by package and name only.
    selected_targets = set()
    for target in targets_json.read_json()["targets"]:
        selected_targets.add(target.split("//")[1])

    linked_target_output_paths = set(linked_targets_json.read_json()["output_paths"]) if linked_targets_json else None

    def is_label_selected(label: Label) -> bool:
        if "{}:{}".format(label.package, label.name) not in selected_targets:
            return False
        return linked_target_output_paths == None or "{}/__{}__".format(label.package, label.name) in linked_target_output_paths

    selected_artifacts = []
    seen = set()

    def add(artifact: Artifact):
        if artifact not in seen:
            seen.add(artifact)
            selected_artifacts.append(artifact)

    if debug_info_tset._tset:
        # The infos of a node come from a single binary, in topological order.
        for infos in debug_info_tset._tset.traverse():
            for info in _select_debug_infos(infos, is_label_selected):
                for artifact in info.artifacts:
                    add(artifact)
            if linked_targets_json:
                # `dsymutil` copies the Swift modules of all the linked targets
                # (the `N_AST` entries of the binary) into the dSYM, whichever
                # objects it links the debug info of.
                for info in infos:
                    if ArtifactInfoTag("swift_debug_info") in info.tags:
                        for artifact in info.artifacts:
                            if artifact.extension == ".swiftmodule":
                                add(artifact)

    entries = {"{}_{}".format(index, artifact.basename): artifact for index, artifact in enumerate(selected_artifacts)}
    entries[_SELECTED_PATHS_FILE_NAME] = actions.write(paths, selected_artifacts)
    actions.symlinked_dir(artifacts, entries)
    return []

_select_debug_info = dynamic_actions(
    impl = _select_debug_info_impl,
    attrs = {
        "artifacts": dynattrs.output(),
        "debug_info_tset": dynattrs.value(ArtifactTSet),
        "linked_targets_json": dynattrs.option(dynattrs.artifact_value()),
        "paths": dynattrs.output(),
        "targets_json": dynattrs.artifact_value(),
    },
)

def apple_selective_debugging_impl(ctx: AnalysisContext) -> list[Provider]:
    json_type = _SelectiveDebuggingJsonType(ctx.attrs.json_type)
    filtering_strategy = _SelectiveDebuggingFilteringStrategy(ctx.attrs.filtering_strategy)

    # process inputs and provide them up the graph with typing
    include_build_target_patterns = [parse_build_target_pattern(pattern) for pattern in ctx.attrs.include_build_target_patterns]
    include_regular_expressions = [
        # TODO(nga): fancy is probably not needed here.
        regex(expression, fancy = True)
        for expression in ctx.attrs.include_regular_expressions
    ]
    exclude_build_target_patterns = [parse_build_target_pattern(pattern) for pattern in ctx.attrs.exclude_build_target_patterns]
    exclude_regular_expressions = [
        # TODO(nga): fancy is probably not needed here.
        regex(expression, fancy = True)
        for expression in ctx.attrs.exclude_regular_expressions
    ]

    scrubber = ctx.attrs._apple_tools[AppleToolsInfo].selective_debugging_scrubber
    dsym_allowlist_tool = ctx.attrs._apple_tools[AppleToolsInfo].selective_debugging_dsym_allowlist_tool

    targets_json_file = None
    selection_args = cmd_args()
    if json_type == _SelectiveDebuggingJsonType("targets"):
        targets_json_file = ctx.attrs.targets_json_file or ctx.actions.write_json("targets.json", {"targets": []}, has_content_based_path = False)

        # If a targets json file is not provided, write an empty json file:
        selection_args.add("--targets-file")
        selection_args.add(targets_json_file)
    elif json_type == _SelectiveDebuggingJsonType("spec"):
        json_data = {
            "exclude_build_target_patterns": ctx.attrs.exclude_build_target_patterns,
            "exclude_regular_expressions": ctx.attrs.exclude_regular_expressions,
            "include_build_target_patterns": ctx.attrs.include_build_target_patterns,
            "include_regular_expressions": ctx.attrs.include_regular_expressions,
        }
        spec_file = ctx.actions.write_json("selective_debugging_spec.json", json_data, has_content_based_path = False)
        selection_args.add("--spec-file")
        selection_args.add(spec_file)
    else:
        fail("Expected json_type to be either `targets` or `spec`.")
    cmd = cmd_args(scrubber, selection_args)

    selection_criteria = _SelectionCriteria(
        include_build_target_patterns = include_build_target_patterns,
        include_regular_expressions = include_regular_expressions,
        exclude_build_target_patterns = exclude_build_target_patterns,
        exclude_regular_expressions = exclude_regular_expressions,
    )

    def scrub_selected_debug_paths_file(inner_ctx: AnalysisContext, package_names: list[str], output_name: str) -> Artifact:
        # In the event that _SelectiveDebuggingJsonType was "spec", we expect that `package_names`
        # was already filtered as part of scrubbing the binary in the apple_bundle.
        #
        # See `_maybe_scrub_binary()` in apple_bundle.bzl
        if json_type != _SelectiveDebuggingJsonType("targets"):
            return inner_ctx.actions.write(output_name, sorted(set(package_names)), has_content_based_path = False)

        def scrub_selected_debug_paths_action(dynamic_ctx: AnalysisContext, artifacts, outputs):
            packages = [
                # "cell//path/to/some/thing:target" -> "path/to/some/thing"
                target.split("//")[1].split(":")[0]
                for target in artifacts[targets_json_file].read_json()["targets"]
            ]
            dynamic_ctx.actions.write(
                outputs.values()[0],
                sorted(set(filter(lambda p: p in packages, package_names))),
            )

        output = inner_ctx.actions.declare_output(output_name, has_content_based_path = False)
        inner_ctx.actions.dynamic_output(
            dynamic = [targets_json_file],
            inputs = [],
            outputs = [output.as_output()],
            f = scrub_selected_debug_paths_action,
        )

        return output

    def select_debug_info(
        inner_ctx: AnalysisContext,
        debug_info_tset: ArtifactTSet,
        output_name: str,
        dsym_binary: Artifact | None = None,
        dsym_binary_execution_preference: LinkExecutionPreference = LinkExecutionPreference("any"),
    ) -> AppleSelectedDebugInfo:
        linked_targets_json = None
        if dsym_binary:
            linked_targets_json = inner_ctx.actions.declare_output(output_name + ".linked_targets.json", has_content_based_path = False)
            action_execution_properties = get_action_execution_attributes(dsym_binary_execution_preference)
            inner_ctx.actions.run(
                cmd_args(dsym_allowlist_tool, "--input", dsym_binary, "--linked-targets-output", linked_targets_json.as_output()),
                category = "selective_debugging_linked_targets",
                identifier = output_name,
                prefer_local = action_execution_properties.prefer_local,
                prefer_remote = action_execution_properties.prefer_remote,
                local_only = action_execution_properties.local_only,
                force_full_hybrid_if_capable = action_execution_properties.full_hybrid,
            )
        artifacts = inner_ctx.actions.declare_output(output_name, dir = True, has_content_based_path = False)
        paths = inner_ctx.actions.declare_output(output_name + ".paths", has_content_based_path = False)
        inner_ctx.actions.dynamic_output_new(
            _select_debug_info(
                artifacts = artifacts.as_output(),
                debug_info_tset = debug_info_tset,
                linked_targets_json = linked_targets_json,
                paths = paths.as_output(),
                targets_json = targets_json_file,
            )
        )
        return AppleSelectedDebugInfo(artifacts = artifacts, paths = artifacts.project(_SELECTED_PATHS_FILE_NAME))

    def scrub_binary(
        inner_ctx,
        executable: Artifact,
        executable_link_execution_preference: LinkExecutionPreference,
        adhoc_codesign_tool: [RunInfo, None],
        focused_targets_labels: list[Label],
        identifier: None | str = None,
    ) -> Artifact:
        inner_cmd = cmd_args(cmd)
        subdir = "{}/".format(identifier) if identifier else ""
        output = inner_ctx.actions.declare_output("debug_scrubbed/{}{}".format(subdir, executable.short_path), has_content_based_path = False)

        action_execution_properties = get_action_execution_attributes(executable_link_execution_preference)

        # If we're provided a codesign tool, provider it to the scrubber binary so that it may sign
        # the binary after scrubbing.
        if adhoc_codesign_tool and (not _should_skip_adhoc_sign_after_scrubbing(inner_ctx)):
            inner_cmd.add(["--adhoc-codesign-tool", adhoc_codesign_tool])
        inner_cmd.add(["--input", executable])
        inner_cmd.add(["--output", output.as_output()])
        _add_persisted_targets_args(inner_ctx, inner_cmd, focused_targets_labels)
        inner_ctx.actions.run(
            inner_cmd,
            category = "scrub_binary",
            identifier = output.short_path,
            prefer_local = action_execution_properties.prefer_local,
            prefer_remote = action_execution_properties.prefer_remote,
            local_only = action_execution_properties.local_only,
            force_full_hybrid_if_capable = action_execution_properties.full_hybrid,
        )
        return output

    def filter_debug_info(inner_ctx: AnalysisContext, debug_info: TransitiveSetIterator) -> AppleSelectiveDebuggingFilteredDebugInfo:
        artifact_infos = []
        selected_target_infos = []
        linked_targets = set()
        is_any_selected_target_linked = False
        is_using_spec = json_type == _SelectiveDebuggingJsonType("spec")
        selected_targets_contain_swift = False
        for infos in debug_info:
            for info in infos:
                is_swift_debug_info = ArtifactInfoTag("swift_debug_info") in info.tags
                is_label_included = _is_label_included(info.label, selection_criteria)

                is_any_selected_target_linked_when_using_spec = is_using_spec and is_any_selected_target_linked
                if not is_any_selected_target_linked_when_using_spec:
                    # When using spec mode and there's already a selected target, there's no need
                    # to continue the search whether a selected target is linked - we know at least
                    # one is already linked. So, the if statement acts as a short-circuit for perf
                    # reasons to avoid unnecessary work.
                    #
                    # In targets mode (i.e., non-spec mode), the full list of `linked_targets` must
                    # be computed, as that value is used behind a dynamic output, so it's not possible
                    # terminate the search early (as we cannot determine whether a selected target is linked
                    # as the list of selected targets is stored in the targets JSON file, which is not available
                    # at analysis time, only avail behind a dynamic output).
                    debug_artifact_contains_object_code = lazy.is_any(
                        lambda debug_artifact: debug_artifact.extension in _OBJECT_FILE_EXTENSIONS, info.artifacts
                    )
                    if debug_artifact_contains_object_code:
                        if is_using_spec and is_label_included:
                            is_any_selected_target_linked = True
                        if not is_using_spec:
                            # `linked_targets` only used in targets mode, avoid costs in all other modes
                            linked_targets.add(info.label)

                if is_label_included:
                    # `selected_target_infos` should only include targets explicitly selected by the user,
                    # not anything included in addition to support the debugger (e.g., `.swiftmodule` files)
                    selected_target_infos.append(info)
                if is_label_included or (selected_targets_contain_swift and is_swift_debug_info):
                    # There might be a few ArtifactInfo corresponding to the same Label,
                    # so to avoid overwriting, we need to preserve all artifacts.
                    artifact_infos.append(info)
                    selected_targets_contain_swift = selected_targets_contain_swift or ArtifactInfoTag("swift_debug_info") in info.tags

        if json_type == _SelectiveDebuggingJsonType("spec"):
            metadata_output = inner_ctx.actions.write_json(
                "selective_metadata_with_spec.json",
                _generate_metadata_json_object(is_any_selected_target_linked),
                pretty = True,
                has_content_based_path = False,
            )
        elif json_type == _SelectiveDebuggingJsonType("targets"):

            def generate_metadata_output(dynamic_ctx: AnalysisContext, artifacts, outputs):
                targets = artifacts[targets_json_file].read_json()["targets"]
                is_any_selected_target_linked_inner = False
                for target in targets:
                    cell, package_with_target_name = target.split("//")
                    package, target_name = package_with_target_name.split(":")

                    is_any_selected_target_linked_inner = lazy.is_any(
                        lambda linked_target: linked_target.cell == cell and linked_target.package == package and linked_target.name == target_name,
                        linked_targets,
                    )
                    if is_any_selected_target_linked_inner:
                        break

                dynamic_ctx.actions.write_json(
                    outputs.values()[0],
                    _generate_metadata_json_object(is_any_selected_target_linked_inner),
                    pretty = True,
                )

            metadata_output = inner_ctx.actions.declare_output("selective_metadata_with_targets_file.json", has_content_based_path = False)
            inner_ctx.actions.dynamic_output(
                dynamic = [targets_json_file],
                inputs = [],
                outputs = [metadata_output.as_output()],
                f = generate_metadata_output,
            )
        else:
            fail("Unexpected type: {}".format(json_type))

        return AppleSelectiveDebuggingFilteredDebugInfo(
            infos = artifact_infos,
            selected_target_infos = selected_target_infos,
            swift_modules_labels = [],
            metadata = metadata_output,
        )

    def generate_dsym_allowlist(
        inner_ctx,
        executable: Artifact,
        executable_link_execution_preference: LinkExecutionPreference,
        focused_targets_labels: list[Label],
    ) -> Artifact:
        output = inner_ctx.actions.declare_output("selective_debugging/{}.dsym_allowlist.yaml".format(executable.short_path), has_content_based_path = False)
        inner_cmd = cmd_args(dsym_allowlist_tool, selection_args, "--input", executable, "--output", output.as_output())
        _add_persisted_targets_args(inner_ctx, inner_cmd, focused_targets_labels)
        action_execution_properties = get_action_execution_attributes(executable_link_execution_preference)
        inner_ctx.actions.run(
            inner_cmd,
            category = "selective_debugging_dsym_allowlist",
            identifier = output.short_path,
            prefer_local = action_execution_properties.prefer_local,
            prefer_remote = action_execution_properties.prefer_remote,
            local_only = action_execution_properties.local_only,
            force_full_hybrid_if_capable = action_execution_properties.full_hybrid,
        )
        return output

    def preference_for_links(links: list[Label], deps_preferences: list[LinkExecutionPreferenceInfo]) -> LinkExecutionPreference:
        # If any dependent links were run locally, prefer that the current link is also performed locally,
        # to avoid needing to upload the previous link.
        dep_prefered_local = lazy.is_any(lambda info: info.preference == LinkExecutionPreference("local"), deps_preferences)
        if dep_prefered_local:
            return LinkExecutionPreference("local")

        # If we're not provided a list of links, we can't make an informed determination.
        if not links:
            return LinkExecutionPreference("any")

        matching_links = filter(None, [link for link in links if _is_label_included(link, selection_criteria)])

        # If more than 20% of targets being linked are also downloaded for debugging, perform the
        # link locally, as we'd need to download the object files anyway (and can skip downloading the link output).
        # Otherwise, perform the link remotely, and we'll just download the debug data separately.
        if len(matching_links) / len(links) >= _LOCAL_LINK_THRESHOLD:
            return LinkExecutionPreference("local")
        return LinkExecutionPreference("remote")

    return [
        DefaultInfo(),
        AppleSelectiveDebuggingInfo(
            scrub_binary = scrub_binary,
            filter = filter_debug_info,
            scrub_selected_debug_paths_file = scrub_selected_debug_paths_file,
            generate_dsym_allowlist = generate_dsym_allowlist if filtering_strategy == _SelectiveDebuggingFilteringStrategy("dsym_allowlist") else None,
            select_debug_info = select_debug_info if json_type == _SelectiveDebuggingJsonType("targets") else None,
        ),
        LinkExecutionPreferenceDeterminatorInfo(preference_for_links = preference_for_links),
    ]

def _add_persisted_targets_args(ctx: AnalysisContext, cmd: cmd_args, focused_targets_labels: list[Label]):
    if len(focused_targets_labels) > 0:
        additional_labels_json = ctx.actions.write_json(
            ctx.attrs.name + ".additional_labels.json",
            {"targets": [label.raw_target() for label in focused_targets_labels]},
            has_content_based_path = False,
        )
        cmd.add(["--persisted-targets-file", additional_labels_json])

def _is_label_included(label: Label, selection_criteria: _SelectionCriteria) -> bool:
    # If no include criteria are provided, we then include everything, as long as it is not excluded.
    if selection_criteria.include_build_target_patterns or selection_criteria.include_regular_expressions:
        if not _check_if_label_matches_patterns_or_expressions(
            label, selection_criteria.include_build_target_patterns, selection_criteria.include_regular_expressions
        ):
            return False

    # If included (above snippet), ensure that this target is not excluded.
    return not _check_if_label_matches_patterns_or_expressions(
        label, selection_criteria.exclude_build_target_patterns, selection_criteria.exclude_regular_expressions
    )

def _check_if_label_matches_patterns_or_expressions(label: Label, patterns: list[BuildTargetPattern], expressions: list[regex]) -> bool:
    for pattern in patterns:
        if pattern.matches(label):
            return True
    for expression in expressions:
        if expression.match(str(label)):
            return True
    return False
