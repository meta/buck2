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

_FORMAT_VERSION = 1


def _flatten_signing_contexts_from_signing_context_tree(
    current_path, signing_context_tree, flattened_contexts
):
    inner_contexts = signing_context_tree.get("inner_signing_contexts", {})
    for inner_rel_path, inner_signing_context_tree in inner_contexts.items():
        inner_path = os.path.join(current_path, inner_rel_path)
        # Process inner bundles first (i.e., inside out) for consistency with codesigning manifest tree
        _flatten_signing_contexts_from_signing_context_tree(
            current_path=inner_path,
            signing_context_tree=inner_signing_context_tree,
            flattened_contexts=flattened_contexts,
        )

    signing_context_path = signing_context_tree.get("signing_context")
    if signing_context_path:
        with open(signing_context_path, "r") as signing_context_file:
            signing_context = json.load(signing_context_file)

            signing_context_version = signing_context["version"]
            if signing_context_version != _FORMAT_VERSION:
                raise RuntimeError(
                    f"Expected format version {_FORMAT_VERSION} but got {signing_context_version}"
                )
            if len(signing_context) > 1:
                # Skip inclusion of empty contexts
                processed_signing_context = dict(signing_context)
                processed_signing_context.pop("version")
                processed_signing_context["path"] = current_path
                flattened_contexts.append(processed_signing_context)


def _args_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        description="Tool which postprocesses a signing context tree to include all information inline."
    )
    parser.add_argument(
        "--signing-context-tree",
        metavar="<signing-context-tree.json>",
        type=Path,
        required=True,
        help="Path to the preprocessed signing context.",
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

    with open(args.signing_context_tree, "r") as signing_context_tree_file:
        signing_context_tree = json.load(signing_context_tree_file)

        flattened_contexts = []
        _flatten_signing_contexts_from_signing_context_tree(
            current_path="",
            signing_context_tree=signing_context_tree,
            flattened_contexts=flattened_contexts,
        )

        with open(args.output, "w") as output_file:
            json.dump(
                {
                    "version": _FORMAT_VERSION,
                    "provisioned_bundles": flattened_contexts,
                },
                output_file,
                indent=4,
            )


if __name__ == "__main__":
    _main()
