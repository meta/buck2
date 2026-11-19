# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

import argparse
import json
import os
from pathlib import Path


def _extract_codesign_invocations_from_codesign_manifest_tree(
    current_path, codesign_manifest_tree, invocations
):
    inner_manifests = codesign_manifest_tree.get("inner_codesign_manifests", {})
    for inner_rel_path, inner_codesign_manifest_tree in inner_manifests.items():
        inner_path = os.path.join(current_path, inner_rel_path)
        # Process inner bundles first (i.e., inside out), as signing outer bundles
        # requires inner bundles to have already been signed (otherwise the outer
        # signature would be invalidated if the inner bundle is signed afterwards).
        _extract_codesign_invocations_from_codesign_manifest_tree(
            current_path=inner_path,
            codesign_manifest_tree=inner_codesign_manifest_tree,
            invocations=invocations,
        )

    codesign_manifest_path = codesign_manifest_tree.get("codesign_manifest")
    if codesign_manifest_path:
        with open(codesign_manifest_path, "r") as codesign_manifest_file:
            codesign_manifest = json.load(codesign_manifest_file)
            codesign_manifest_invocations = codesign_manifest.get("invocations", [])
            for original_manifest_invocation in codesign_manifest_invocations:
                original_invocation_path = original_manifest_invocation["path"]
                updated_invocation_path = (
                    os.path.join(current_path, original_invocation_path)
                    if original_invocation_path != "."
                    else current_path
                )

                updated_manifest_invocation = dict(original_manifest_invocation)
                updated_manifest_invocation["path"] = updated_invocation_path

                invocations.append(updated_manifest_invocation)


def _args_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        description="Tool which postprocesses a codesign manifest tree."
    )
    parser.add_argument(
        "--codesign-manifest-tree",
        metavar="<codesign_manifest_tree.json>",
        type=Path,
        required=True,
        help="Path to the [codesign-manifest-tree] subtarget output.",
    )
    parser.add_argument(
        "--output",
        metavar="<output.json>",
        type=Path,
        required=True,
        help="Path to the postprocessed file.",
    )

    return parser


def _main() -> None:
    args_parser = _args_parser()
    args = args_parser.parse_args()

    with open(args.codesign_manifest_tree, "r") as codesign_manifest_tree_file:
        codesign_manifest_tree = json.load(codesign_manifest_tree_file)

        invocations = []
        _extract_codesign_invocations_from_codesign_manifest_tree(
            current_path="",
            codesign_manifest_tree=codesign_manifest_tree,
            invocations=invocations,
        )

        with open(args.output, "w") as output_file:
            json.dump(
                {
                    "version": 1,
                    "invocations": invocations,
                },
                output_file,
                indent=4,
            )


if __name__ == "__main__":
    _main()
