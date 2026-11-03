# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

from __future__ import annotations

import argparse
import cProfile
import json
import logging
import os
import pstats
import subprocess
import tempfile
from pathlib import Path
from typing import List, Optional

from apple.tools.code_signing.codesign_bundle import (
    codesign_bundle,
    CodesignConfiguration,
    CodesignedPath,
    write_empty_codesign_manifest,
)
from apple.tools.code_signing.signing_context_types import (
    AdhocSigningContext,
    selection_profile_context_from_signing_context,
    SigningContextWithProfileSelection,
)
from apple.tools.re_compatibility_utils.writable import make_dir_recursively_writable

from .action_metadata import action_metadata_if_present
from .assemble_bundle import assemble_bundle
from .assemble_bundle_types import BundleSpecItem, IncrementalContext
from .incremental_state import (
    IncrementalState,
    IncrementalStateItem,
    IncrementalStateJSONEncoder,
    parse_incremental_state,
)
from .incremental_utils import codesigned_on_copy_item
from .logging_utils import configure_logging
from .signing_context import (
    add_args_for_bundling_execution,
    add_args_for_signing_context_path,
)
from .signing_context_data import (
    load_signing_context_data_from_file,
    SigningContextData,
)
from .swift_support import run_swift_stdlib_tool, SwiftSupportArguments


_METADATA_PATH_KEY = "ACTION_METADATA"


def _args_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        description="Tool which assembles the Apple bundle."
    )
    parser.add_argument(
        "--output",
        metavar="</path/to/app.bundle>",
        type=Path,
        required=True,
        help="Absolute path to Apple bundle result.",
    )
    parser.add_argument(
        "--spec",
        metavar="<Spec.json>",
        type=Path,
        required=True,
        help="Path to file with JSON representing the bundle contents. It should contain a dictionary which maps bundle relative destination paths to source paths.",
    )
    parser.add_argument(
        "--codesign-tool",
        metavar="<codesign>",
        type=Path,
        required=False,
        help="Path to code signing utility. If not provided standard `codesign` tool will be used.",
    )
    parser.add_argument(
        "--codesign-manifest",
        metavar="<manifest_file>",
        type=Path,
        required=False,
        help="Path to a file containing codesign invocations.",
    )
    parser.add_argument(
        "--codesign-args",
        type=str,
        default=[],
        required=False,
        action="append",
        help="Add additional args to pass during codesigning. Pass as`--codesign-args=ARG` to ensure correct arg parsing.",
    )
    parser.add_argument(
        "--codesign-configuration",
        required=False,
        type=CodesignConfiguration,
        choices=[e.value for e in CodesignConfiguration],
        help=f"""
            Augments how code signing is run.
            Pass `{CodesignConfiguration.fastAdhoc}` to skip adhoc signing bundles if the executables are already adhoc signed.
            Pass `{CodesignConfiguration.executionBypass}` to skip executing the actual codesigning commands.
            Pass `{CodesignConfiguration.dryRun}` for code signing to be run in dry mode (instead of actual signing only .plist
            files with signing parameters will be generated in the root of each signed bundle).
        """,
    )
    parser.add_argument(
        "--fast-adhoc-signing-probe-enabled",
        action="store_true",
        help="Allow fast-adhoc signing to consult the bundled signature probe tool when it is available.",
    )
    parser.add_argument(
        "--incremental-state",
        metavar="<IncrementalState.json>",
        type=Path,
        required=False,
        help="Required if script is run in incremental mode. Path to file with JSON which describes the contents of bundle built previously.",
    )
    parser.add_argument(
        "--profile-output",
        metavar="<ProfileOutput.txt>",
        type=Path,
        required=False,
        help="Path to the profiling output. If present profiling will be enabled.",
    )
    parser.add_argument(
        "--log-level-stderr",
        choices=["debug", "info", "warning", "error", "critical"],
        type=str,
        required=False,
        default="warning",
        help="Logging level for messages written to stderr.",
    )
    parser.add_argument(
        "--log-level-file",
        choices=["debug", "info", "warning", "error", "critical"],
        type=str,
        required=False,
        default="info",
        help="Logging level for messages written to a log file.",
    )
    parser.add_argument(
        "--binary-destination",
        metavar="<Binary>",
        type=Path,
        required=False,
        help="Required if swift support was requested. Bundle relative destination path to bundle binary.",
    )
    parser.add_argument(
        "--frameworks-destination",
        metavar="<Frameworks>",
        type=Path,
        required=False,
        help="Required if swift support was requested. Bundle relative destination path to frameworks directory.",
    )
    parser.add_argument(
        "--extensionkit-extensions-destination",
        metavar="<ExtensionKitExtensions>",
        type=Path,
        required=False,
        help="Required if swift support was requested. Bundle relative destination path to ExtensionKit Extensions directory.",
    )
    parser.add_argument(
        "--plugins-destination",
        metavar="<Plugins>",
        type=Path,
        required=False,
        help="Required if swift support was requested. Bundle relative destination path to plugins directory.",
    )
    parser.add_argument(
        "--resources-destination",
        metavar="<Resources>",
        type=Path,
        required=False,
        help="Required if swift support was requested. Bundle relative destination path to resources directory.",
    )
    parser.add_argument(
        "--appclips-destination",
        metavar="<AppClips>",
        type=Path,
        required=False,
        help="Required if swift support was requested. Bundle relative destination path to appclips directory.",
    )
    parser.add_argument(
        "--sdk-root",
        metavar="<path/to/SDK>",
        type=Path,
        required=False,
        help="Required if swift support was requested. Path to SDK root.",
    )
    parser.add_argument(
        "--swift-stdlib-command",
        metavar='<"/swift/stdlib/tool --foo bar/qux">',
        type=str,
        required=False,
        help="Swift stdlib command prefix. If present, output bundle will contain needed Swift standard libraries (to support the lack of ABI stability or certain backports usage).",
    )
    parser.add_argument(
        "--check-conflicts",
        action="store_true",
        help="Check there are no path conflicts between different source parts of the bundle if enabled.",
    )
    parser.add_argument(
        "--versioned-if-macos",
        action="store_true",
        help="Create symlinks for versioned macOS bundle",
    )
    parser.add_argument(
        "--include-build-info-file",
        action="store_true",
        help="Include the build info file in the assembled bundle.",
    )
    parser.add_argument(
        "--build-info-swift-dynamic-replacement",
        choices=["true", "false"],
        help="Record in the build info file whether the configuration compiles Swift for dynamic replacement.",
    )
    parser.add_argument(
        "--bundle-telemetry-logger",
        metavar="<path/to/logger>",
        type=Path,
        required=False,
        help="Path to bundle telemetry logger tool. If provided, will be invoked after bundle assembly completes.",
    )

    add_args_for_bundling_execution(parser)
    add_args_for_signing_context_path(parser)

    return parser


def _get_codesigned_paths_for_spec_item(
    bundle_path: CodesignedPath,
    args: argparse.Namespace,
    item: BundleSpecItem,
    codesign_configuration: Optional[CodesignConfiguration],
) -> List[CodesignedPath]:
    if not item.codesign_on_copy:
        return []

    entitlements = (
        Path(item.codesign_entitlements) if item.codesign_entitlements else None
    )
    flags = (
        item.codesign_flags_override
        if (item.codesign_flags_override is not None)
        else args.codesign_args
    )

    codesigned_paths = []
    extra_file_paths = []

    is_dry_run = codesign_configuration is CodesignConfiguration.dryRun
    extra_codesign_paths = item.extra_codesign_paths or []
    for extra_codesign_path in extra_codesign_paths:
        path = bundle_path.path / item.dst / extra_codesign_path
        if not path.exists():
            raise RuntimeError(
                f"Found non-existing extra path to codesign: {extra_codesign_path} for {item.src}"
            )

        if path.is_file() and is_dry_run:
            # In dry-run mode, non-bundle items should be signed as part of the containing bundle.
            extra_file_paths.append(extra_codesign_path)
        else:
            codesigned_paths.append(
                CodesignedPath(
                    path=path,
                    entitlements=entitlements,
                    flags=flags,
                    extra_file_paths=None,
                )
            )

    codesigned_paths.append(
        CodesignedPath(
            path=bundle_path.path / item.dst,
            entitlements=entitlements,
            flags=flags,
            extra_file_paths=extra_file_paths,
        )
    )

    return codesigned_paths


def _get_codesigned_paths_from_spec(
    bundle_path: CodesignedPath,
    args: argparse.Namespace,
    spec: List[BundleSpecItem],
) -> List[CodesignedPath]:
    codesigned_paths = []
    for item in spec:
        codesigned_paths += _get_codesigned_paths_for_spec_item(
            bundle_path=bundle_path,
            args=args,
            item=item,
            codesign_configuration=args.codesign_configuration,
        )
    return codesigned_paths


def _load_signing_context(
    args: argparse.Namespace,
    temp_dir: Path,
) -> tuple[AdhocSigningContext | SigningContextWithProfileSelection | None, str | None]:
    if not args.codesign:
        return None, None

    signing_context_data = load_signing_context_data_from_file(
        args.signing_context_path
    )
    _materialize_selected_profile(signing_context_data, temp_dir)
    return signing_context_data.signing_context, signing_context_data.selected_identity


def _main(spec_temp_dir: str) -> None:
    args_parser = _args_parser()
    args = args_parser.parse_args()

    uname_info = os.uname()
    sysname = uname_info.sysname
    major_release = int(uname_info.release.split(".")[0])

    # macOS 26.0.0 Tahoe no longer supports SHA1 as an option in the codesign tool.
    # Note: This is safe to remove along with the code that adds the digest-algorithm code
    # in apple_bundle_wrapping_rule.bzl once we are migrated to macOS 26.0.
    if (
        "--digest-algorithm=sha1" in args.codesign_args
        and sysname == "Darwin"
        and major_release
        >= 25  # Darwin kernel release 25.0.0 corresponds to macOS 26.0.0 Tahoe
    ):
        args.codesign_args.remove("--digest-algorithm=sha1")

    configure_logging(
        stderr_level=getattr(logging, args.log_level_stderr.upper()),
        file_level=getattr(logging, args.log_level_file.upper()),
        log_path=args.log_file,
    )

    pr = cProfile.Profile()
    profiling_enabled = args.profile_output is not None
    if profiling_enabled:
        pr.enable()

    signing_context, selected_identity_argument = _load_signing_context(
        args,
        Path(spec_temp_dir),
    )

    with args.spec.open(mode="rb") as spec_file:
        spec = json.load(spec_file, object_hook=lambda d: BundleSpecItem(**d))
        spec = _deduplicate_spec(spec)

    incremental_context = _incremental_context(
        incremenatal_state_path=args.incremental_state,
        codesigned=args.codesign,
        codesign_configuration=args.codesign_configuration,
        codesign_identity=selected_identity_argument,
        codesign_arguments=args.codesign_args,
        versioned_if_macos=args.versioned_if_macos,
    )

    if args.include_build_info_file:
        _amend_spec_with_build_uuid(
            spec=spec,
            resources_destination=args.resources_destination,
            tmp_dir=Path(spec_temp_dir),
            swift_dynamic_replacement=args.build_info_swift_dynamic_replacement,
        )

    incremental_state = assemble_bundle(
        spec=spec,
        bundle_path=args.output,
        incremental_context=incremental_context,
        check_conflicts=args.check_conflicts,
        versioned_if_macos=args.versioned_if_macos,
    )

    swift_support_args = _swift_support_arguments(
        args_parser,
        args,
    )

    if swift_support_args:
        swift_stdlib_paths = run_swift_stdlib_tool(
            bundle_path=args.output,
            args=swift_support_args,
        )
    else:
        swift_stdlib_paths = []

    prepared_entitlements_path: Optional[Path] = None
    telemetry_tmp_dir: Optional[tempfile.TemporaryDirectory[str]] = None

    if args.codesign:
        # Vendored frameworks/bundles could already be pre-signed, in which case,
        # re-signing them requires modifying them. On RE, the umask is such that
        # copied files (when constructing the bundle) are not writable.
        make_dir_recursively_writable(args.output)
        if signing_context is None:
            raise RuntimeError(
                "Expected signing context to be created before bundling is done if codesign is requested."
            )

        bundle_path = CodesignedPath(
            path=args.output,
            entitlements=args.entitlements,
            flags=args.codesign_args,
            extra_file_paths=None,
        )
        codesign_on_copy_paths = _get_codesigned_paths_from_spec(
            bundle_path=bundle_path, spec=spec, args=args
        ) + [
            CodesignedPath(
                path=bundle_path.path / path,
                entitlements=None,
                flags=args.codesign_args,
                extra_file_paths=None,
            )
            for path in swift_stdlib_paths
        ]

        if args.bundle_telemetry_logger:
            telemetry_tmp_dir = tempfile.TemporaryDirectory()
            prepared_entitlements_path = (
                Path(telemetry_tmp_dir.name) / "prepared_entitlements.plist"
            )

        codesign_bundle(
            bundle_path=bundle_path,
            signing_context=signing_context,
            platform=args.platform,
            codesign_on_copy_paths=codesign_on_copy_paths,
            codesign_tool=args.codesign_tool,
            codesign_configuration=args.codesign_configuration,
            fast_adhoc_signing_probe_enabled=args.fast_adhoc_signing_probe_enabled,
            codesign_manifest_path=args.codesign_manifest,
            entitlements_suffixed_key_map=args.entitlements_suffixed_key_map,
            entitlements_removed_keys=args.entitlements_removed_keys,
            entitlements_removed_values_map=args.entitlements_removed_values_map,
            prepared_entitlements_output_path=prepared_entitlements_path,
        )
    elif args.codesign_manifest:
        # Always write the codesign manifest file, even when unsigned
        write_empty_codesign_manifest(
            codesign_manifest_path=args.codesign_manifest,
            bundle_path=args.output,
        )

    if incremental_state:
        if incremental_context is None:
            raise RuntimeError(
                "Expected incremental context to be present when incremental state is non-null."
            )
        _write_incremental_state(
            spec=spec,
            items=incremental_state,
            path=args.incremental_state,
            codesigned=args.codesign,
            codesign_configuration=args.codesign_configuration,
            selected_codesign_identity=selected_identity_argument,
            codesign_arguments=args.codesign_args,
            swift_stdlib_paths=swift_stdlib_paths,
            versioned_if_macos=args.versioned_if_macos,
            incremental_context=incremental_context,
        )

    if profiling_enabled:
        pr.disable()
        with open(args.profile_output, "w") as s:
            sortby = pstats.SortKey.CUMULATIVE
            ps = pstats.Stats(pr, stream=s).sort_stats(sortby)
            ps.print_stats()

    if args.bundle_telemetry_logger:
        selected_profile_path = _get_selected_profile_path(signing_context)
        info_plist_in_bundle = (
            (args.output / args.info_plist_destination)
            if args.info_plist_destination
            else None
        )
        _run_bundle_telemetry_logger(
            logger_path=args.bundle_telemetry_logger,
            bundle_path=args.output,
            info_plist=info_plist_in_bundle,
            entitlements=prepared_entitlements_path,
            selected_provisioning_profile=selected_profile_path,
        )

    if telemetry_tmp_dir is not None:
        telemetry_tmp_dir.cleanup()


def _materialize_selected_profile(
    signing_context_data: SigningContextData,
    output_dir: Path,
) -> None:
    profile_context = selection_profile_context_from_signing_context(
        signing_context_data.signing_context
    )
    if profile_context is None:
        return
    if signing_context_data.provisioning_profile_data is None:
        raise ValueError("Signing context is missing profile data")

    profile = profile_context.selected_profile_info.profile
    materialized_path = output_dir / profile.file_path.name
    materialized_path.write_bytes(signing_context_data.provisioning_profile_data)
    profile.file_path = materialized_path


def _get_selected_profile_path(
    signing_context: Optional[object],
) -> Optional[Path]:
    if signing_context is None:
        return None

    signing_context_with_profile: Optional[SigningContextWithProfileSelection] = None
    if isinstance(signing_context, SigningContextWithProfileSelection):
        signing_context_with_profile = signing_context
    if isinstance(signing_context, AdhocSigningContext):
        signing_context_with_profile = signing_context.profile_selection_context

    return (
        signing_context_with_profile.selected_profile_info.profile.file_path
        if signing_context_with_profile
        else None
    )


def _run_bundle_telemetry_logger(
    logger_path: Path,
    bundle_path: Path,
    info_plist: Optional[Path],
    entitlements: Optional[Path],
    selected_provisioning_profile: Optional[Path],
) -> None:
    cmd: List[str] = [str(logger_path), "--bundle", str(bundle_path)]
    if info_plist:
        cmd.extend(["--info-plist", str(info_plist)])
    if entitlements:
        cmd.extend(["--entitlements", str(entitlements)])
    if selected_provisioning_profile:
        cmd.extend(["--provisioning-profile", str(selected_provisioning_profile)])
    try:
        subprocess.run(cmd, check=False)
    except Exception:
        # Telemetry is best-effort, never fail the build
        logging.getLogger(__name__).debug(
            "Failed to run bundle telemetry logger",
            exc_info=True,
        )


def _incremental_context(
    incremenatal_state_path: Optional[Path],
    codesigned: bool,
    codesign_configuration: CodesignConfiguration,
    codesign_identity: Optional[str],
    codesign_arguments: List[str],
    versioned_if_macos: bool,
) -> Optional[IncrementalContext]:
    action_metadata = action_metadata_if_present(_METADATA_PATH_KEY)
    if action_metadata is None:
        # Environment variable not set, running in non-incremental mode.
        return None
    # If there is no incremental state or we failed to parse it (maybe because of a format change)
    # do a clean (non-incremental) assemble right now but generate proper state for next run.
    incremental_state = (
        _read_incremental_state(incremenatal_state_path)
        if incremenatal_state_path
        else None
    )
    return IncrementalContext(
        metadata=action_metadata,
        state=incremental_state,
        codesigned=codesigned,
        codesign_configuration=codesign_configuration,
        codesign_identity=codesign_identity,
        codesign_arguments=codesign_arguments,
        versioned_if_macos=versioned_if_macos,
    )


def _read_incremental_state(path: Path) -> Optional[IncrementalState]:
    logging.getLogger(__name__).info(f"Will read incremental state from `{path}`.")
    if not path.exists():
        logging.getLogger(__name__).warning(
            f"File with incremental state doesn't exist at `{path}`."
        )
        return None
    try:
        with path.open() as f:
            return parse_incremental_state(f)
    except Exception:
        logging.getLogger(__name__).exception("Failed to read incremental state")
        return None
    finally:
        # If something goes wrong and we don't delete the file
        # we probably end up in faulty state where incremental state
        # doesn't match the output. Hence delete it early.
        path.unlink()


def _swift_support_arguments(
    parser: argparse.ArgumentParser,
    args: argparse.Namespace,
) -> Optional[SwiftSupportArguments]:
    if not args.swift_stdlib_command:
        return None
    if not args.binary_destination:
        parser.error(
            "Expected `--binary-destination` argument to be specified when `--swift-stdlib-command` is present."
        )
    if not args.appclips_destination:
        parser.error(
            "Expected `--appclips-destination` argument to be specified when `--swift-stdlib-command` is present."
        )
    if not args.frameworks_destination:
        parser.error(
            "Expected `--frameworks-destination` argument to be specified when `--swift-stdlib-command` is present."
        )
    if not args.extensionkit_extensions_destination:
        parser.error(
            "Expected `--extensionkit-extensions-destination` argument to be specified when `--swift-stdlib-command` is present."
        )
    if not args.plugins_destination:
        parser.error(
            "Expected `--plugins-destination` argument to be specified when `--swift-stdlib-command` is present."
        )
    if not args.platform:
        parser.error(
            "Expected `--platform` argument to be specified when `--swift-stdlib-command` is present."
        )
    if not args.sdk_root:
        parser.error(
            "Expected `--sdk-root` argument to be specified when `--swift-stdlib-command` is present."
        )
    return SwiftSupportArguments(
        swift_stdlib_command=args.swift_stdlib_command,
        binary_destination=args.binary_destination,
        appclips_destination=args.appclips_destination,
        frameworks_destination=args.frameworks_destination,
        extensionkit_extensions_destination=args.extensionkit_extensions_destination,
        plugins_destination=args.plugins_destination,
        platform=args.platform,
        sdk_root=args.sdk_root,
    )


def _write_incremental_state(
    spec: List[BundleSpecItem],
    items: List[IncrementalStateItem],
    path: Path,
    codesigned: bool,
    codesign_configuration: CodesignConfiguration,
    selected_codesign_identity: Optional[str],
    codesign_arguments: List[str],
    swift_stdlib_paths: List[Path],
    versioned_if_macos: bool,
    incremental_context: IncrementalContext,
) -> None:
    state = IncrementalState(
        items,
        codesigned=codesigned,
        codesign_configuration=codesign_configuration,
        codesigned_on_copy=[
            codesigned_on_copy_item(
                path=Path(i.dst),
                entitlements=(
                    Path(i.codesign_entitlements) if i.codesign_entitlements else None
                ),
                incremental_context=incremental_context,
                codesign_flags_override=i.codesign_flags_override,
                extra_codesign_paths=i.extra_codesign_paths,
            )
            for i in spec
            if i.codesign_on_copy
        ],
        codesign_identity=selected_codesign_identity,
        codesign_arguments=codesign_arguments,
        swift_stdlib_paths=swift_stdlib_paths,
        versioned_if_macos=versioned_if_macos,
    )
    path.touch()
    try:
        with path.open(mode="w") as f:
            json.dump(state, f, cls=IncrementalStateJSONEncoder)
    except Exception:
        path.unlink()
        raise


def _amend_spec_with_build_uuid(
    spec: List[BundleSpecItem],
    resources_destination: Optional[Path],
    tmp_dir: Path,
    swift_dynamic_replacement: Optional[str] = None,
) -> None:
    build_id = os.environ.get("BUCK_BUILD_ID")
    if not build_id:
        return

    if resources_destination is None:
        raise RuntimeError(
            "`--resources-destination` is required when `--include-build-info-file` is set."
        )

    # There caveats when the build info would be updated - specifically, it would
    # be updated whenever the action runs. When the action runs is more tricky
    # to precisely specify but practically speaking, if the bundling action does
    # not run due to behing a cache hit, the build uuid would be from the last
    # build that produced the cache value.
    #
    # For example, if you did the following:
    #  - `buck build //my:app` (build uuid X)
    #  - `buck build //my:app` (build uuid Y)
    #
    # Build Y runs no actions at all (nothing invalidated because synthesized
    # files are not inputs to the action), so `BuildInfo.json` would contain
    # uuid X.
    build_info = {"build_uuid": build_id}
    if swift_dynamic_replacement is not None:
        build_info["swift_dynamic_replacement"] = swift_dynamic_replacement == "true"
    src = tmp_dir / "BuildInfo.json"
    src.write_text(json.dumps(build_info, indent=2))
    dst = str(resources_destination / "BuildInfo.json")

    spec.append(BundleSpecItem(src=str(src), dst=dst))


def _deduplicate_spec(spec: List[BundleSpecItem]) -> List[BundleSpecItem]:
    # It's possible to have the same spec multiple times as different
    # apple_resource() targets can refer to the _same_ resource file.
    #
    # On RE, we're not allowed to overwrite files, so prevent doing
    # identical file copies.
    #
    # Do not reorder spec items to achieve determinism.
    # Rely on the fact that `dict` preserves key order.
    deduplicated_spec = list(dict.fromkeys(spec))
    # Force same sorting as in Buck1 for `SourcePathWithAppleBundleDestination`
    # WARNING: This logic is tightly coupled with how spec filtering is done in `_filter_conflicting_paths` method during incremental bundling. Don't change unless you fully understand what is going on here.
    deduplicated_spec.sort()
    return deduplicated_spec


if __name__ == "__main__":
    with tempfile.TemporaryDirectory() as spec_temp_dir:
        _main(spec_temp_dir)
