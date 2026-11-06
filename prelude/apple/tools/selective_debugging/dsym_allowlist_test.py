# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

from __future__ import annotations

import json
import os
import tempfile
import unittest
from unittest.mock import patch

import importlib_resources as resources

from .dsym_allowlist import (
    kept_debug_file_paths,
    linked_target_output_paths,
    write_dsymutil_allowlist,
)
from .dsym_allowlist_main import main
from .scrubber import scrub

# `HelloWorld` links three object files directly and one object file from a
# static archive.
_PREFIX = (
    "buck-out/v2/gen/fbsource/56628b5feecfab0a/fbobjc/buck2/samples/focused_debugging"
)
_ARCHIVE_MEMBER = f"{_PREFIX}/__Foo__/libFoo.a(Foo.mm.o)"
_OBJECTS = [
    f"{_PREFIX}/__HelloWorld__/__objects__/srcs/AppDelegate.m.o",
    f"{_PREFIX}/__HelloWorld__/__objects__/srcs/RootViewController.m.o",
    f"{_PREFIX}/__HelloWorld__/__objects__/srcs/main.m.o",
]

# The output directories of the targets in the debug map of `HelloWorld`. Each
# path is also read as a content-based path, whose configuration hash then
# looks like the first component of the package.
_LINKED_TARGET_OUTPUT_PATHS = sorted(
    [
        f"{prefix}fbobjc/buck2/samples/focused_debugging/{path}"
        for prefix in ["", "56628b5feecfab0a/"]
        for path in ["__Foo__", "__HelloWorld__", "__HelloWorld__/__objects__"]
    ]
)

_TARGETS_FILES = [
    "focused_targets_empty.json",
    "focused_targets.json",
    "focused_targets_objects.json",
]
_SPEC_FILES = [
    "focused_spec.json",
    "focused_spec_regex_all.json",
    "focused_spec_regex_none.json",
]


def _resource(name: str) -> str:
    return str(resources.files(__package__).joinpath(f"test_resources/{name}"))


class DsymAllowlistTest(unittest.TestCase):
    def setUp(self) -> None:
        self.tmp_dir = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp_dir.cleanup)

    def _kept(
        self, targets_file: str | None = None, spec_file: str | None = None
    ) -> list[str]:
        return kept_debug_file_paths(
            _resource("HelloWorld"),
            None,
            targets_file=_resource(targets_file) if targets_file else None,
            spec_file=_resource(spec_file) if spec_file else None,
        )

    def _kept_by_scrubber(
        self, targets_file: str | None = None, spec_file: str | None = None
    ) -> list[str]:
        results = scrub(
            _resource("HelloWorld"),
            os.path.join(self.tmp_dir.name, "scrubbed"),
            None,
            targets_file=_resource(targets_file) if targets_file else None,
            spec_file=_resource(spec_file) if spec_file else None,
        )
        return sorted(
            {original for original, rewritten in results if original == rewritten}
        )

    def test_focused_archive_member(self) -> None:
        self.assertEqual(
            self._kept(targets_file="focused_targets.json"), [_ARCHIVE_MEMBER]
        )
        self.assertEqual(self._kept(spec_file="focused_spec.json"), [_ARCHIVE_MEMBER])

    def test_focused_object_files(self) -> None:
        self.assertEqual(
            self._kept(targets_file="focused_targets_objects.json"), _OBJECTS
        )

    def test_everything_or_nothing_focused(self) -> None:
        self.assertEqual(
            self._kept(spec_file="focused_spec_regex_all.json"),
            sorted(_OBJECTS + [_ARCHIVE_MEMBER]),
        )
        self.assertEqual(self._kept(spec_file="focused_spec_regex_none.json"), [])
        self.assertEqual(self._kept(targets_file="focused_targets_empty.json"), [])

    def test_keeps_exactly_what_the_scrubber_keeps(self) -> None:
        for targets_file in _TARGETS_FILES:
            with self.subTest(targets_file=targets_file):
                self.assertEqual(
                    self._kept(targets_file=targets_file),
                    self._kept_by_scrubber(targets_file=targets_file),
                )
        for spec_file in _SPEC_FILES:
            with self.subTest(spec_file=spec_file):
                self.assertEqual(
                    self._kept(spec_file=spec_file),
                    self._kept_by_scrubber(spec_file=spec_file),
                )

    def test_input_is_not_modified(self) -> None:
        with open(_resource("HelloWorld"), "rb") as f:
            original = f.read()
        self._kept(spec_file="focused_spec_regex_none.json")
        with open(_resource("HelloWorld"), "rb") as f:
            self.assertEqual(f.read(), original)

    def test_write_allowlist(self) -> None:
        output = os.path.join(self.tmp_dir.name, "allow.yaml")
        write_dsymutil_allowlist(["a/libFoo.a(Foo.o)", "b/it's: #odd.o"], output)
        with open(output) as f:
            self.assertEqual(
                json.load(f),
                {
                    "objects": [
                        {"filename": "a/libFoo.a(Foo.o)"},
                        {"filename": "b/it's: #odd.o"},
                    ]
                },
            )

    def test_main(self) -> None:
        output = os.path.join(self.tmp_dir.name, "allow.yaml")
        argv = [
            "dsym_allowlist",
            "--input",
            _resource("HelloWorld"),
            "--output",
            output,
            "--targets-file",
            _resource("focused_targets.json"),
        ]
        with patch("sys.argv", argv):
            main()

        with open(output) as f:
            self.assertEqual(json.load(f), {"objects": [{"filename": _ARCHIVE_MEMBER}]})

    def test_linked_target_output_paths(self) -> None:
        self.assertEqual(
            linked_target_output_paths(_resource("HelloWorld")),
            _LINKED_TARGET_OUTPUT_PATHS,
        )

    def test_main_linked_targets(self) -> None:
        output = os.path.join(self.tmp_dir.name, "linked_targets.json")
        argv = [
            "dsym_allowlist",
            "--input",
            _resource("HelloWorld"),
            "--linked-targets-output",
            output,
        ]
        with patch("sys.argv", argv):
            main()

        with open(output) as f:
            self.assertEqual(
                json.load(f),
                {"output_paths": _LINKED_TARGET_OUTPUT_PATHS},
            )

    def test_main_linked_targets_rejects_selection(self) -> None:
        argv = [
            "dsym_allowlist",
            "--input",
            _resource("HelloWorld"),
            "--linked-targets-output",
            os.path.join(self.tmp_dir.name, "linked_targets.json"),
            "--targets-file",
            _resource("focused_targets.json"),
        ]
        with patch("sys.argv", argv), self.assertRaises(SystemExit):
            main()
