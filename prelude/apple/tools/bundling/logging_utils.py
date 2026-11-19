# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

from __future__ import annotations

import logging
import sys
from pathlib import Path


def configure_logging(
    stderr_level: int,
    file_level: int,
    log_path: Path | None,
) -> None:
    stderr_handler = logging.StreamHandler()
    stderr_handler.setLevel(stderr_level)
    log_format = (
        "%(asctime)s - %(name)s - %(levelname)s - %(message)s (%(filename)s:%(lineno)d)"
    )
    stderr_handler.setFormatter(
        _ColoredLogFormatter(log_format)
        if sys.stderr.isatty()
        else logging.Formatter(log_format)
    )

    handlers: list[logging.Handler] = [stderr_handler]
    if log_path is not None:
        # Declared log outputs must exist even when no messages are emitted, and
        # incremental runs must not retain messages from an earlier invocation.
        log_path.write_text("")
        file_handler = logging.FileHandler(log_path, encoding="utf-8")
        file_handler.setFormatter(logging.Formatter(log_format))
        file_handler.setLevel(file_level)
        handlers.append(file_handler)

    logging.basicConfig(level=logging.DEBUG, handlers=handlers)


class _ColoredLogFormatter(logging.Formatter):
    _colors: dict[int, str] = {
        logging.DEBUG: "\x1b[m",
        logging.INFO: "\x1b[37m",
        logging.WARNING: "\x1b[33m",
        logging.ERROR: "\x1b[31m",
        logging.CRITICAL: "\x1b[1;31m",
    }
    _reset_color = "\x1b[0m"

    def __init__(self, text_format: str) -> None:
        self.text_format = text_format

    def format(self, record: logging.LogRecord) -> str:
        formatter = logging.Formatter(
            self._colors.get(record.levelno, "") + self.text_format + self._reset_color
        )
        return formatter.format(record)
