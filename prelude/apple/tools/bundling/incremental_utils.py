# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

# pyre-strict

import hashlib
import logging
import os
from pathlib import Path
from typing import Dict, List, Optional, Set, Tuple

from .assemble_bundle_types import BundleSpecItem, IncrementalContext
from .incremental_state import CodesignedOnCopy, IncrementalStateItem

FILES_TO_BE_IGNORED: Set[str] = {
    # Storage of Finder settings, which shouldn't be added when enumerating files from sources
    ".DS_Store",
}


def should_assemble_incrementally(
    spec: List[BundleSpecItem], incremental_context: IncrementalContext
) -> bool:
    previous_run_state = incremental_context.state
    if previous_run_state is None:
        logging.getLogger(__name__).info(
            "Decided not to assemble incrementally — no incremental state for previous build."
        )
        return False
    if previous_run_state.versioned_if_macos != incremental_context.versioned_if_macos:
        logging.getLogger(__name__).info(
            "Decided not to assemble incrementally — current build and previous build have different versioned_if_macos settings."
        )
        return False

    previously_codesigned = previous_run_state.codesigned
    # If previously bundle was not code signed there should be no problems with code signing
    # currently in incremental mode. Existing binaries could be code signed "on
    # top" if that's needed.
    if not previously_codesigned:
        logging.getLogger(__name__).info(
            "Decided to assemble incrementally — previous build is not codesigned."
        )
        return True
    # For simplicity and correctness purposes instead of stripping code signatures we
    # perform non-incremental run.
    if not incremental_context.codesigned:
        logging.getLogger(__name__).info(
            "Decided not to assemble incrementally — current build is not codesigned, while previous build is codesigned."
        )
        return False
    # If previous identity is different from the current one also perform non-incremental run.
    if previous_run_state.codesign_identity != incremental_context.codesign_identity:
        logging.getLogger(__name__).info(
            "Decided not to assemble incrementally — previous vs current builds have mismatching codesigning identities."
        )
        return False
    # If previous codesign arguments are different from the current ones also perform non-incremental run.
    if previous_run_state.codesign_arguments != incremental_context.codesign_arguments:
        logging.getLogger(__name__).info(
            "Decided not to assemble incrementally — previous vs current builds have mismatching codesigning arguments."
        )
        return False
    # If bundle from previous run was signed in a different configuration vs the current run (e.g. dry code signed while now regular code signing is required) perform non-incremental run.
    if (
        previous_run_state.codesign_configuration
        != incremental_context.codesign_configuration
    ):
        logging.getLogger(__name__).info(
            "Decided not to assemble incrementally — previous vs current builds have mismatching codesigning configurations."
        )
        return False
    # If there is an artifact that was code signed on copy in previous run which is
    # present in current run and not code signed on copy, we should perform
    # non-incremental run for simplicity and correctness reasons.
    current_codesigned_on_copy_items = {
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
    }

    codesigned_on_copy_paths_from_previous_build_which_are_present_in_current_build = _codesigned_on_copy_paths_from_previous_build_which_are_present_in_current_build(
        previous_run_state.codesigned_on_copy,
        {Path(i.dst) for i in spec},
    )
    codesign_on_copy_paths_are_compatible = codesigned_on_copy_paths_from_previous_build_which_are_present_in_current_build.issubset(
        current_codesigned_on_copy_items
    )
    if not codesign_on_copy_paths_are_compatible:
        logging.getLogger(__name__).info(
            f"Decided not to assemble incrementally — there is at least one artifact `{list(codesigned_on_copy_paths_from_previous_build_which_are_present_in_current_build - current_codesigned_on_copy_items)[0]}` that was code signed on copy in previous build which is present in current run and not code signed on copy (or codesigned but with a different set of entitlements, flags or extra paths.)."
        )
    return codesign_on_copy_paths_are_compatible


def _codesigned_on_copy_paths_from_previous_build_which_are_present_in_current_build(
    previously_codesigned_on_copy: List[CodesignedOnCopy],
    all_input_files: Set[Path],
) -> Set[CodesignedOnCopy]:
    all_input_files_and_directories = all_input_files | {
        i for file in all_input_files for i in file.parents
    }
    return {
        i
        for i in previously_codesigned_on_copy
        if i.path in all_input_files_and_directories
    }


def _get_new_digest(action_metadata: Dict[str, str], path: Path) -> str:
    # A spec source is normally an action input recorded under exactly this
    # path, and this runs once per file in the bundle, so try the lookup before
    # resolving: `Path.resolve()` walks and stats every component of the path.
    digest = action_metadata.get(str(path))
    if digest is not None:
        return digest

    # While a resource file can be in a symlinked folder, like the `ghi/def` example below,
    # ```
    # project_root
    #           ├── abc
    #           │    └── def
    #           └── ghi -> abc
    # ```
    # In this case, Python would say `ghi/abc` not a symlink. However the `action_metadata` comes
    # with the actual resolved path (`abc/def`). We need to resolve the path then.
    # Given Python doesn't think it's a symlink, the `readlink` API wouldn't work either
    resolved_path = path.resolve()

    cwd = Path.cwd()
    try:
        path_relative_to_cwd = resolved_path.relative_to(cwd)
    except ValueError:
        # Synthesized paths are those generated by the bundling script itself
        # (i.e., not an input to the action).
        #
        # Need to handle the case where synthesized path (usually in a temp dir)
        # cannot be relativized against the cwd
        path_relative_to_cwd = None

    if path_relative_to_cwd is not None:
        digest = action_metadata.get(str(path_relative_to_cwd))
        if digest is not None:
            return digest

    # The bundler synthesizes some files itself (e.g. build info) rather than
    # them being action inputs, so they have no entry in the action metadata.
    hasher = hashlib.sha256()
    with open(resolved_path, "rb") as f:
        for chunk in iter(lambda: f.read(1024 * 1024), b""):
            hasher.update(chunk)
    return hasher.hexdigest()


def calculate_incremental_state(
    spec: List[BundleSpecItem], action_metadata: Dict[str, str]
) -> List[IncrementalStateItem]:
    """
    `action_metadata` maps Buck project relative paths to hash digest
    for every input file of the action which executes this script
    """
    result = []
    source_with_destination_files = _source_with_destination_files(spec)
    for src, dst in source_with_destination_files:
        is_symlink = src.is_symlink()
        new_digest = _get_new_digest(action_metadata, src) if not is_symlink else None
        resolved_symlink = Path(os.readlink(src)) if is_symlink else None
        result.append(
            IncrementalStateItem(
                source=src,
                destination_relative_to_bundle=dst,
                digest=new_digest,
                resolved_symlink=resolved_symlink,
            )
        )
    return result


def _source_with_destination_files(
    spec: List[BundleSpecItem],
) -> List[Tuple[Path, Path]]:
    """
    Returns:
        Ordered mapping from source path to destination path (relative to bundle) for every file
        present in a bundle. Directories that were parts of the spec are split into actual files.
    """
    result = []
    for spec_item in spec:
        file_or_dir = Path(spec_item.src)
        if file_or_dir.is_file():
            if not spec_item.dst:
                raise RuntimeError(
                    f'Invalid input bundle spec. File located at {file_or_dir} should not have `""` destination (only directories are allowed to have such value).'
                )
            result.append((file_or_dir, Path(spec_item.dst)))
        elif file_or_dir.is_dir():
            result.extend(
                _list_directory_deterministically(spec_item.src, spec_item.dst)
            )
        else:
            raise RuntimeError(
                f"Path {file_or_dir} is not a file and not a dir, don't know how to handle it."
            )

    return [(src, dst) for src, dst in result if src.name not in FILES_TO_BE_IGNORED]


def _list_directory_deterministically(
    directory: str, destination_prefix: str
) -> List[Tuple[Path, Path]]:
    result = []
    for current_dir_path, dir_names, file_names in os.walk(directory):
        relative_dir = os.path.relpath(current_dir_path, directory)
        destination_dir = os.path.join(destination_prefix, relative_dir)
        # `os.walk` lists a symlink to a directory among the directories but does not
        # descend into it, so it is an entry of the bundle like a file.
        linked_dir_names = [
            dir_name
            for dir_name in dir_names
            if os.path.islink(os.path.join(current_dir_path, dir_name))
        ]
        result += [
            (
                Path(os.path.join(current_dir_path, file_name)),
                Path(os.path.join(destination_dir, file_name)),
            )
            for file_name in sorted(file_names + linked_dir_names)
        ]
        # Sort in order for walk to be deterministic.
        dir_names[:] = sorted(set(dir_names) - set(linked_dir_names))
    return result


def codesigned_on_copy_item(
    path: Path,
    entitlements: Optional[Path],
    incremental_context: IncrementalContext,
    codesign_flags_override: Optional[List[str]],
    extra_codesign_paths: Optional[List[str]],
) -> CodesignedOnCopy:
    if entitlements is not None:
        digest = incremental_context.metadata.get(str(entitlements))
        if digest is None:
            raise RuntimeError(
                f"Expected digest for entitlements file path `{entitlements}` to be present in action metadata."
            )
    else:
        digest = None
    return CodesignedOnCopy(
        path=path,
        entitlements_digest=digest,
        codesign_flags_override=codesign_flags_override,
        extra_codesign_paths=extra_codesign_paths,
    )
