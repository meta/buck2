# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

import argparse
import logging
from pathlib import Path

from .logging_utils import configure_logging
from .signing_context import (
    add_args_for_signing_context,
    signing_context_and_selected_identity_from_args,
)
from .signing_context_data import write_signing_context_data_to_file


def _args_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        description="Tool which resolves signing context and caches it for downstream bundling and signing_context tools.",
    )
    parser.add_argument(
        "--output",
        required=True,
        type=Path,
        help="Path to the output JSON file containing the signing context.",
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

    add_args_for_signing_context(parser)

    return parser


def _main() -> None:
    parser = _args_parser()
    args = parser.parse_args()

    configure_logging(
        stderr_level=getattr(logging, args.log_level_stderr.upper()),
        file_level=getattr(logging, args.log_level_file.upper()),
        log_path=args.log_file,
    )

    signing_context, selected_identity = (
        signing_context_and_selected_identity_from_args(args)
    )

    write_signing_context_data_to_file(
        path=args.output,
        signing_context=signing_context,
        selected_identity=selected_identity,
    )


if __name__ == "__main__":
    _main()
