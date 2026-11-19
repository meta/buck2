# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

import pathlib
import unittest
from tempfile import NamedTemporaryFile
from typing import Optional
from unittest.mock import call, patch

import importlib_resources as resources

from .scrubber import (
    load_focused_targets_output_paths,
    scrub,
    should_scrub_with_focused_targets_output_paths,
)

FAKE_PATH = "fake/path"


class Test(unittest.TestCase):
    def test_no_focused_targets(self):
        results, _ = _get_scrubber_results(targets_json_file_path=None)
        for _, rewrite_path in results:
            # We expect all paths to be scrubbed
            self.assertEqual(rewrite_path.strip(), FAKE_PATH)

    def test_empty_focused_targets(self):
        results, _ = _get_scrubber_results(
            targets_json_file_path="focused_targets_empty.json"
        )
        for _, rewrite_path in results:
            # We expect all paths to be scrubbed
            self.assertEqual(rewrite_path.strip(), FAKE_PATH)

    def test_focused_targets(self):
        with resources.as_file(
            _get_test_resource_file("focused_targets.json")
        ) as targets_json_file:
            output_paths = load_focused_targets_output_paths(str(targets_json_file))

        results, _ = _get_scrubber_results(
            targets_json_file_path="focused_targets.json"
        )

        focused_paths = []
        scrubbed_paths = []

        for orig_path, rewrite_path in results:
            for output_path in output_paths:
                if output_path in orig_path:
                    # Ensure we didn't scrub the path
                    focused_paths.append(orig_path)
                    self.assertEqual(orig_path, rewrite_path)
                else:
                    # Ensure we scrubbed the path
                    scrubbed_paths.append(orig_path)
                    self.assertEqual(rewrite_path.strip(), FAKE_PATH)

        self.assertEqual(
            focused_paths,
            [
                "buck-out/v2/gen/fbsource/56628b5feecfab0a/fbobjc/buck2/samples/focused_debugging/__Foo__/libFoo.a(Foo.mm.o)",
            ],
        )
        self.assertEqual(
            scrubbed_paths,
            [
                "buck-out/v2/gen/fbsource/56628b5feecfab0a/fbobjc/buck2/samples/focused_debugging/__HelloWorld__/__objects__/srcs/AppDelegate.m.o",
                "buck-out/v2/gen/fbsource/56628b5feecfab0a/fbobjc/buck2/samples/focused_debugging/__HelloWorld__/__objects__/srcs/RootViewController.m.o",
                "buck-out/v2/gen/fbsource/56628b5feecfab0a/fbobjc/buck2/samples/focused_debugging/__HelloWorld__/__objects__/srcs/main.m.o",
            ],
        )

    def test_spec_targets(self):
        focused_paths, scrubbed_paths = _get_focused_and_scrubbed_paths(
            spec_json_file_path="focused_spec.json"
        )

        self.assertEqual(
            focused_paths,
            [
                "buck-out/v2/gen/fbsource/56628b5feecfab0a/fbobjc/buck2/samples/focused_debugging/__Foo__/libFoo.a(Foo.mm.o)",
            ],
        )
        self.assertEqual(
            scrubbed_paths,
            [
                "buck-out/v2/gen/fbsource/56628b5feecfab0a/fbobjc/buck2/samples/focused_debugging/__HelloWorld__/__objects__/srcs/AppDelegate.m.o",
                "buck-out/v2/gen/fbsource/56628b5feecfab0a/fbobjc/buck2/samples/focused_debugging/__HelloWorld__/__objects__/srcs/RootViewController.m.o",
                "buck-out/v2/gen/fbsource/56628b5feecfab0a/fbobjc/buck2/samples/focused_debugging/__HelloWorld__/__objects__/srcs/main.m.o",
            ],
        )

    @patch("subprocess.run")
    def test_codesigning(self, subprocess):
        subprocess.return_value = [0, 0]
        results, output_file = _get_scrubber_results(
            spec_json_file_path="focused_spec.json",
            adhoc_codesign_tool="/usr/fake/codesign",
        )
        expected_calls = [
            call(
                [
                    "/usr/fake/codesign",
                    "--binary",
                    output_file,
                ],
                check=True,
            ),
        ]
        subprocess.assert_has_calls(expected_calls)

    def test_load_focused_targets_output_paths(self):
        with resources.as_file(
            _get_test_resource_file("focused_targets.json")
        ) as targets_json_file:
            output_paths = load_focused_targets_output_paths(str(targets_json_file))

        self.assertEqual(
            output_paths, {"fbobjc/buck2/samples/focused_debugging/__Foo__"}
        )

    def test_spec_targets_regex_all(self):
        focused_paths, scrubbed_paths = _get_focused_and_scrubbed_paths(
            spec_json_file_path="focused_spec_regex_all.json"
        )
        self.assertEqual(
            focused_paths,
            [
                "buck-out/v2/gen/fbsource/56628b5feecfab0a/fbobjc/buck2/samples/focused_debugging/__HelloWorld__/__objects__/srcs/AppDelegate.m.o",
                "buck-out/v2/gen/fbsource/56628b5feecfab0a/fbobjc/buck2/samples/focused_debugging/__HelloWorld__/__objects__/srcs/RootViewController.m.o",
                "buck-out/v2/gen/fbsource/56628b5feecfab0a/fbobjc/buck2/samples/focused_debugging/__HelloWorld__/__objects__/srcs/main.m.o",
                "buck-out/v2/gen/fbsource/56628b5feecfab0a/fbobjc/buck2/samples/focused_debugging/__Foo__/libFoo.a(Foo.mm.o)",
            ],
        )
        self.assertEqual(
            scrubbed_paths,
            [],
        )

    def test_spec_targets_regex_none(self):
        focused_paths, scrubbed_paths = _get_focused_and_scrubbed_paths(
            spec_json_file_path="focused_spec_regex_none.json"
        )

        self.assertEqual(
            focused_paths,
            [],
        )
        self.assertEqual(
            scrubbed_paths,
            [
                "buck-out/v2/gen/fbsource/56628b5feecfab0a/fbobjc/buck2/samples/focused_debugging/__HelloWorld__/__objects__/srcs/AppDelegate.m.o",
                "buck-out/v2/gen/fbsource/56628b5feecfab0a/fbobjc/buck2/samples/focused_debugging/__HelloWorld__/__objects__/srcs/RootViewController.m.o",
                "buck-out/v2/gen/fbsource/56628b5feecfab0a/fbobjc/buck2/samples/focused_debugging/__HelloWorld__/__objects__/srcs/main.m.o",
                "buck-out/v2/gen/fbsource/56628b5feecfab0a/fbobjc/buck2/samples/focused_debugging/__Foo__/libFoo.a(Foo.mm.o)",
            ],
        )

    def test_should_scrub_with_focused_targets_output_paths(self):
        focused_targets_output_paths = {
            "fbobjc/some/path/__foo__",
            "xplat/some/path/__foo__",
        }
        self.assertEqual(
            True,
            should_scrub_with_focused_targets_output_paths(
                focused_targets_output_paths,
                "buck-out/v2/gen/fbsource/56628b5feecfab0a/fbobjc/some/path/__baz__/libbar.a(baz.mm.o)",
            ),
        )
        self.assertEqual(
            True,
            should_scrub_with_focused_targets_output_paths(
                focused_targets_output_paths,
                "buck-out/v2/gen/fbsource/56628b5feecfab0a/fbobjc/some/path/__baz__/__objects__/baz.mm.o",
            ),
        )
        self.assertEqual(
            True,
            should_scrub_with_focused_targets_output_paths(
                focused_targets_output_paths,
                "buck-out/v2/gen/fbsource/fbobjc/some/path/__baz__/__objects__/56628b5feecfab0a/baz.mm.o",
            ),
        )
        self.assertEqual(
            False,
            should_scrub_with_focused_targets_output_paths(
                focused_targets_output_paths,
                "buck-out/v2/gen/fbsource/56628b5feecfab0a/fbobjc/some/path/__foo__/libbar.a(baz.mm.o)",
            ),
        )
        self.assertEqual(
            False,
            should_scrub_with_focused_targets_output_paths(
                focused_targets_output_paths,
                "buck-out/v2/gen/fbsource/56628b5feecfab0a/fbobjc/some/path/__foo__/__objects__/baz.mm.o",
            ),
        )
        self.assertEqual(
            False,
            should_scrub_with_focused_targets_output_paths(
                focused_targets_output_paths,
                "buck-out/v2/gen/fbsource/56628b5feecfab0a/fbobjc/some/path/__foo__/lib.a",
            ),
        )
        self.assertEqual(
            False,
            should_scrub_with_focused_targets_output_paths(
                focused_targets_output_paths,
                "buck-out/v2/gen/fbsource/fbobjc/some/path/__foo__/56628b5feecfab0a/lib.a",
            ),
        )
        self.assertEqual(
            False,
            should_scrub_with_focused_targets_output_paths(
                focused_targets_output_paths,
                "buck-out/v2/gen/fbsource/56628b5feecfab0a/fbobjc/some/path/__foo__/__objects__/bar.o",
            ),
        )
        self.assertEqual(
            False,
            should_scrub_with_focused_targets_output_paths(
                focused_targets_output_paths,
                "buck-out/v2/gen/fbsource/56628b5feecfab0a/fbobjc/some/path/__foo__/__swift_incremental__/objects/Foo.swift.o",
            ),
        )
        self.assertEqual(
            True,
            should_scrub_with_focused_targets_output_paths(
                focused_targets_output_paths,
                "buck-out/v2/gen/fbsource/56628b5feecfab0a/fbobjc/some/path/__baz__/__swift_incremental__/objects/Baz.swift.o",
            ),
        )
        # Directories in the buck-out prefix are not target output directories
        self.assertEqual(
            True,
            should_scrub_with_focused_targets_output_paths(
                {""},
                "buck-out/__v2__/gen/fbsource/56628b5feecfab0a/fbobjc/some/path/__baz__/lib.a",
            ),
        )
        self.assertEqual(
            False,
            should_scrub_with_focused_targets_output_paths(
                focused_targets_output_paths,
                "xplat/some/path/foo/lib/prebuilt_lib.a(baz.m.o)",
            ),
        )
        self.assertEqual(
            True,
            should_scrub_with_focused_targets_output_paths(
                focused_targets_output_paths,
                "xplat/some/path/fooo/prebuilt_lib.a(baz.m.o)",
            ),
        )
        self.assertEqual(
            True,
            should_scrub_with_focused_targets_output_paths(
                focused_targets_output_paths,
                "/tmp/lto.tmp",
            ),
        )
        self.assertEqual(
            True,
            should_scrub_with_focused_targets_output_paths(
                focused_targets_output_paths,
                "/",
            ),
        )


def _get_focused_and_scrubbed_paths(
    targets_json_file_path: Optional[str] = None,
    spec_json_file_path: Optional[str] = None,
    adhoc_codesign_tool: Optional[str] = None,
) -> (list[str], list[str]):
    results, _ = _get_scrubber_results(
        targets_json_file_path=targets_json_file_path,
        spec_json_file_path=spec_json_file_path,
        adhoc_codesign_tool=adhoc_codesign_tool,
    )

    focused_paths = []
    scrubbed_paths = []

    for orig_path, rewrite_path in results:
        if rewrite_path.strip() == FAKE_PATH:
            scrubbed_paths.append(orig_path)
        elif orig_path == rewrite_path:
            focused_paths.append(orig_path)
        else:
            raise Exception(
                f"Rewrite path is neither the fake path nor the original path: {rewrite_path}"
            )

    return (focused_paths, scrubbed_paths)


@patch(
    "apple.tools.selective_debugging.scrubber.make_path_user_writable",
    return_value=None,
)
def _get_scrubber_results(
    make_path_user_writable_mock,
    targets_json_file_path: Optional[str] = None,
    spec_json_file_path: Optional[str] = None,
    adhoc_codesign_tool: Optional[str] = None,
) -> tuple[list[tuple[str, str]], str]:
    with resources.as_file(_get_test_resource_file("HelloWorld")) as test_binary_file:
        with NamedTemporaryFile() as out_file:
            if targets_json_file_path:
                with resources.as_file(
                    _get_test_resource_file(targets_json_file_path)
                ) as targets_json_file:
                    return (
                        scrub(
                            str(test_binary_file),
                            out_file.name,
                            None,
                            targets_file=str(targets_json_file),
                            adhoc_codesign_tool=adhoc_codesign_tool,
                        ),
                        out_file.name,
                    )
            elif spec_json_file_path:
                with resources.as_file(
                    _get_test_resource_file(spec_json_file_path),
                ) as spec_json_file:
                    return (
                        scrub(
                            str(test_binary_file),
                            out_file.name,
                            None,
                            spec_file=str(spec_json_file),
                            adhoc_codesign_tool=adhoc_codesign_tool,
                        ),
                        out_file.name,
                    )
            else:
                return scrub(str(test_binary_file), out_file.name, None), out_file.name


def _get_test_resource_file(name) -> pathlib.Path:
    path = resources.files(__package__).joinpath(f"test_resources/{name}")
    return path
