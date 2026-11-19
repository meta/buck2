# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

import io
import os
import tempfile
import unittest
from pathlib import Path
from typing import Generator
from unittest.mock import patch

from apple.tools.code_signing.codesign_bundle import CodesignConfiguration

from .action_metadata import parse_action_metadata
from .assemble_bundle_types import BundleSpecItem
from .incremental_state import CodesignedOnCopy, IncrementalState, IncrementalStateItem
from .incremental_utils import (
    calculate_incremental_state,
    codesigned_on_copy_item,
    IncrementalContext,
    should_assemble_incrementally,
)

try:
    from contextlib import chdir  # pyre-ignore[21], Python 3.11+
except ImportError:
    from contextlib import contextmanager

    @contextmanager
    def chdir(path: os.PathLike) -> Generator[None, None, None]:
        cwd = os.getcwd()
        try:
            os.chdir(path)
            yield
        finally:
            os.chdir(cwd)


class TestIncrementalUtils(unittest.TestCase):
    maxDiff = None

    def test_not_run_incrementally_when_previous_build_not_incremental(self):
        spec = [
            BundleSpecItem(
                src="src/foo",
                dst="foo",
                codesign_on_copy=False,
            )
        ]
        incremental_context = IncrementalContext(
            metadata={"foo": "digest"},
            state=None,
            codesigned=False,
            codesign_configuration=None,
            codesign_identity=None,
            codesign_arguments=[],
            versioned_if_macos=True,
        )
        self.assertFalse(should_assemble_incrementally(spec, incremental_context))

    def test_run_incrementally_when_previous_build_not_codesigned(self):
        spec = [
            BundleSpecItem(
                src="src/foo",
                dst="foo",
                codesign_on_copy=False,
            )
        ]
        incremental_context = IncrementalContext(
            metadata={"foo": "digest"},
            state=IncrementalState(
                items=[
                    IncrementalStateItem(
                        source=Path("src/foo"),
                        destination_relative_to_bundle=Path("foo"),
                        digest="digest",
                        resolved_symlink=None,
                    )
                ],
                codesigned=False,
                codesign_configuration=None,
                codesigned_on_copy=[],
                codesign_identity=None,
                codesign_arguments=[],
                versioned_if_macos=True,
                swift_stdlib_paths=[],
            ),
            codesigned=True,
            codesign_configuration=None,
            codesign_identity=None,
            codesign_arguments=[],
            versioned_if_macos=True,
        )
        self.assertTrue(should_assemble_incrementally(spec, incremental_context))

    def test_not_run_incrementally_when_previous_build_codesigned_and_current_is_not(
        self,
    ):
        spec = [
            BundleSpecItem(
                src="src/foo",
                dst="foo",
                codesign_on_copy=False,
            )
        ]
        incremental_context = IncrementalContext(
            metadata={"foo": "digest"},
            state=IncrementalState(
                items=[
                    IncrementalStateItem(
                        source=Path("src/foo"),
                        destination_relative_to_bundle=Path("foo"),
                        digest="digest",
                        resolved_symlink=None,
                    )
                ],
                codesigned=True,
                codesign_configuration=None,
                codesigned_on_copy=[],
                codesign_identity=None,
                codesign_arguments=[],
                versioned_if_macos=True,
                swift_stdlib_paths=[],
            ),
            codesigned=False,
            codesign_configuration=None,
            codesign_identity=None,
            codesign_arguments=[],
            versioned_if_macos=True,
        )
        self.assertFalse(should_assemble_incrementally(spec, incremental_context))
        # Check that behavior changes when both builds are codesigned
        incremental_context.codesigned = True
        self.assertTrue(should_assemble_incrementally(spec, incremental_context))

    def test_not_run_incrementally_when_previous_build_codesigned_with_different_identity(
        self,
    ):
        spec = [
            BundleSpecItem(
                src="src/foo",
                dst="foo",
                codesign_on_copy=False,
            )
        ]
        incremental_context = IncrementalContext(
            metadata={"foo": "digest"},
            state=IncrementalState(
                items=[
                    IncrementalStateItem(
                        source=Path("src/foo"),
                        destination_relative_to_bundle=Path("foo"),
                        digest="digest",
                        resolved_symlink=None,
                    )
                ],
                codesigned=True,
                codesign_configuration=None,
                codesigned_on_copy=[],
                codesign_identity="old_identity",
                codesign_arguments=[],
                versioned_if_macos=True,
                swift_stdlib_paths=[],
            ),
            codesigned=True,
            codesign_configuration=None,
            codesign_identity="new_identity",
            codesign_arguments=[],
            versioned_if_macos=True,
        )
        self.assertFalse(should_assemble_incrementally(spec, incremental_context))
        # Check that behavior changes when identities are same
        incremental_context.state.codesign_identity = "same_identity"
        incremental_context.codesign_identity = "same_identity"
        self.assertTrue(should_assemble_incrementally(spec, incremental_context))

    def test_run_incrementally_when_codesign_on_copy_paths_match(self):
        spec = [
            BundleSpecItem(
                src="src/foo",
                dst="foo",
                codesign_on_copy=True,
            ),
            BundleSpecItem(
                src="src/bar",
                dst="bar",
                codesign_on_copy=True,
            ),
            BundleSpecItem(
                src="src/baz",
                dst="baz",
                codesign_on_copy=True,
                codesign_entitlements="entitlements.plist",
            ),
        ]
        incremental_context = IncrementalContext(
            metadata={
                "src/foo": "digest",
                "src/baz": "digest2",
                "entitlements.plist": "entitlements_digest",
            },
            state=IncrementalState(
                items=[
                    IncrementalStateItem(
                        source=Path("src/foo"),
                        destination_relative_to_bundle=Path("foo"),
                        digest="digest",
                        resolved_symlink=None,
                    ),
                    IncrementalStateItem(
                        source=Path("src/baz"),
                        destination_relative_to_bundle=Path("baz"),
                        digest="digest2",
                        resolved_symlink=None,
                    ),
                ],
                codesigned=True,
                codesign_configuration=None,
                codesigned_on_copy=[
                    CodesignedOnCopy(
                        path=Path("foo"),
                        entitlements_digest=None,
                        codesign_flags_override=None,
                        extra_codesign_paths=None,
                    ),
                    CodesignedOnCopy(
                        path=Path("baz"),
                        entitlements_digest="entitlements_digest",
                        codesign_flags_override=None,
                        extra_codesign_paths=None,
                    ),
                ],
                codesign_identity="same_identity",
                codesign_arguments=[],
                versioned_if_macos=True,
                swift_stdlib_paths=[],
            ),
            codesigned=True,
            codesign_configuration=None,
            codesign_identity="same_identity",
            codesign_arguments=[],
            versioned_if_macos=True,
        )
        self.assertTrue(should_assemble_incrementally(spec, incremental_context))

    def test_not_run_incrementally_when_codesign_on_copy_paths_mismatch(self):
        spec = [
            BundleSpecItem(
                src="src/foo",
                dst="foo",
                # want it to be not codesigned in new build
                codesign_on_copy=False,
            )
        ]
        incremental_context = IncrementalContext(
            metadata={"src/foo": "digest"},
            state=IncrementalState(
                items=[
                    IncrementalStateItem(
                        source=Path("src/foo"),
                        destination_relative_to_bundle=Path("foo"),
                        digest="digest",
                        resolved_symlink=None,
                    )
                ],
                codesigned=True,
                codesign_configuration=None,
                # but it was codesigned in old build
                codesigned_on_copy=[
                    CodesignedOnCopy(
                        path=Path("foo"),
                        entitlements_digest=None,
                        codesign_flags_override=None,
                        extra_codesign_paths=None,
                    )
                ],
                codesign_identity="same_identity",
                codesign_arguments=[],
                versioned_if_macos=True,
                swift_stdlib_paths=[],
            ),
            codesigned=True,
            codesign_configuration=None,
            codesign_identity="same_identity",
            codesign_arguments=[],
            versioned_if_macos=True,
        )
        self.assertFalse(should_assemble_incrementally(spec, incremental_context))
        spec[0].codesign_on_copy = True
        self.assertTrue(should_assemble_incrementally(spec, incremental_context))

    def test_not_run_incrementally_when_codesign_on_copy_entitlements_mismatch(self):
        spec = [
            BundleSpecItem(
                src="src/foo",
                dst="foo",
                codesign_on_copy=True,
                codesign_entitlements="baz/entitlements.plist",
            )
        ]
        incremental_context = IncrementalContext(
            metadata={
                "src/foo": "digest",
                "baz/entitlements.plist": "new_digest",
            },
            state=IncrementalState(
                items=[
                    IncrementalStateItem(
                        source=Path("src/foo"),
                        destination_relative_to_bundle=Path("foo"),
                        digest="digest",
                        resolved_symlink=None,
                    )
                ],
                codesigned=True,
                codesign_configuration=None,
                codesigned_on_copy=[
                    CodesignedOnCopy(
                        path=Path("foo"),
                        entitlements_digest="old_digest",
                        codesign_flags_override=None,
                        extra_codesign_paths=None,
                    )
                ],
                codesign_identity="same_identity",
                codesign_arguments=[],
                versioned_if_macos=True,
                swift_stdlib_paths=[],
            ),
            codesigned=True,
            codesign_configuration=None,
            codesign_identity="same_identity",
            codesign_arguments=[],
            versioned_if_macos=True,
        )
        self.assertFalse(should_assemble_incrementally(spec, incremental_context))
        incremental_context.metadata["baz/entitlements.plist"] = "old_digest"
        self.assertTrue(should_assemble_incrementally(spec, incremental_context))

    def test_not_run_incrementally_when_codesign_on_copy_flags_mismatch(self):
        spec = [
            BundleSpecItem(
                src="src/foo",
                dst="foo",
                codesign_on_copy=True,
                codesign_flags_override=["--force"],
            )
        ]
        incremental_context = IncrementalContext(
            metadata={
                "src/foo": "digest",
            },
            state=IncrementalState(
                items=[
                    IncrementalStateItem(
                        source=Path("src/foo"),
                        destination_relative_to_bundle=Path("foo"),
                        digest="digest",
                        resolved_symlink=None,
                    )
                ],
                codesigned=True,
                codesign_configuration=None,
                codesigned_on_copy=[
                    CodesignedOnCopy(
                        path=Path("foo"),
                        entitlements_digest=None,
                        codesign_flags_override=["--force", "--deep"],
                        extra_codesign_paths=None,
                    )
                ],
                codesign_identity="same_identity",
                codesign_arguments=[],
                versioned_if_macos=True,
                swift_stdlib_paths=[],
            ),
            codesigned=True,
            codesign_configuration=None,
            codesign_identity="same_identity",
            codesign_arguments=[],
            versioned_if_macos=True,
        )
        self.assertFalse(should_assemble_incrementally(spec, incremental_context))
        incremental_context.state.codesigned_on_copy[0].codesign_flags_override = [
            "--force"
        ]
        self.assertTrue(should_assemble_incrementally(spec, incremental_context))

    def test_not_run_incrementally_when_extra_codesign_paths_mismatch(self):
        extra_codesign_paths_before = ["Frameworks/Base.framework"]
        extra_codesign_paths_after = [
            "Frameworks/Base.framework",
            "Frameworks/Extra.framework",
        ]
        spec = [
            BundleSpecItem(
                src="src/foo",
                dst="foo",
                codesign_on_copy=True,
                extra_codesign_paths=extra_codesign_paths_after,
            )
        ]
        incremental_context = IncrementalContext(
            metadata={
                "src/foo": "digest",
            },
            state=IncrementalState(
                items=[
                    IncrementalStateItem(
                        source=Path("src/foo"),
                        destination_relative_to_bundle=Path("foo"),
                        digest="digest",
                        resolved_symlink=None,
                    )
                ],
                codesigned=True,
                codesign_configuration=None,
                codesigned_on_copy=[
                    CodesignedOnCopy(
                        path=Path("foo"),
                        entitlements_digest=None,
                        codesign_flags_override=None,
                        extra_codesign_paths=extra_codesign_paths_before,
                    )
                ],
                codesign_identity="same_identity",
                codesign_arguments=[],
                versioned_if_macos=True,
                swift_stdlib_paths=[],
            ),
            codesigned=True,
            codesign_configuration=None,
            codesign_identity="same_identity",
            codesign_arguments=[],
            versioned_if_macos=True,
        )
        self.assertFalse(should_assemble_incrementally(spec, incremental_context))
        incremental_context.state.codesigned_on_copy[
            0
        ].extra_codesign_paths = extra_codesign_paths_after
        self.assertTrue(should_assemble_incrementally(spec, incremental_context))

    def test_not_run_incrementally_when_codesign_arguments_mismatch(self):
        spec = [
            BundleSpecItem(
                src="src/foo",
                dst="foo",
            )
        ]
        incremental_context = IncrementalContext(
            metadata={
                "src/foo": "digest",
            },
            state=IncrementalState(
                items=[
                    IncrementalStateItem(
                        source=Path("src/foo"),
                        destination_relative_to_bundle=Path("foo"),
                        digest="digest",
                        resolved_symlink=None,
                    )
                ],
                codesigned=True,
                codesign_configuration=None,
                codesigned_on_copy=[],
                codesign_identity="same_identity",
                codesign_arguments=["--force"],
                swift_stdlib_paths=[],
                versioned_if_macos=True,
            ),
            codesigned=True,
            codesign_configuration=None,
            codesign_identity="same_identity",
            codesign_arguments=["--force", "--deep"],
            versioned_if_macos=True,
        )
        self.assertFalse(should_assemble_incrementally(spec, incremental_context))
        incremental_context.codesign_arguments = ["--force"]
        self.assertTrue(should_assemble_incrementally(spec, incremental_context))

    def test_not_run_incrementally_when_codesign_configurations_mismatch(self):
        spec = [
            BundleSpecItem(
                src="src/foo",
                dst="foo",
                codesign_on_copy=True,
            )
        ]
        incremental_context = IncrementalContext(
            metadata={"src/foo": "digest"},
            state=IncrementalState(
                items=[
                    IncrementalStateItem(
                        source=Path("src/foo"),
                        destination_relative_to_bundle=Path("foo"),
                        digest="digest",
                        resolved_symlink=None,
                    )
                ],
                codesigned=True,
                # Dry codesigned in old build
                codesign_configuration=CodesignConfiguration.dryRun,
                codesigned_on_copy=[
                    CodesignedOnCopy(
                        path=Path("foo"),
                        entitlements_digest=None,
                        codesign_flags_override=None,
                        extra_codesign_paths=None,
                    )
                ],
                codesign_identity="same_identity",
                codesign_arguments=[],
                versioned_if_macos=True,
                swift_stdlib_paths=[],
            ),
            codesigned=True,
            codesign_configuration=CodesignConfiguration.dryRun,
            codesign_identity="same_identity",
            codesign_arguments=[],
            versioned_if_macos=True,
        )
        # Canary
        self.assertTrue(should_assemble_incrementally(spec, incremental_context))
        # Now we want a regular signing in new build
        incremental_context.codesign_configuration = None
        self.assertFalse(should_assemble_incrementally(spec, incremental_context))

    def test_calculate_incremental_state(self):
        with tempfile.TemporaryDirectory() as project_root, chdir(project_root):
            # project_root
            #           ├── foo
            #           ├── bar
            #           │    ├── baz
            #           │    └── qux -> baz
            #           ├── abc
            #           │    └── def
            #           └── ghi -> abc
            Path("foo").write_text("hello")
            bar_path = Path("bar")
            bar_path.mkdir()
            (bar_path / "baz").write_text("world")
            (bar_path / "qux").symlink_to("baz")
            abc_path = Path("abc")
            abc_path.mkdir()
            (abc_path / "def").write_text("yo")
            Path("ghi").symlink_to("abc")

            action_metadata = {
                "foo": "hash(foo)",
                "bar/baz": "hash(baz)",
                "abc/def": "hash(def)",
            }
            spec = [
                BundleSpecItem(
                    src="foo",
                    dst="foo",
                    codesign_on_copy=False,
                ),
                BundleSpecItem(
                    src="bar",
                    dst="tux",
                    codesign_on_copy=True,
                ),
                BundleSpecItem(
                    src="ghi",
                    dst="ghi",
                    codesign_on_copy=True,
                ),
            ]
            state = calculate_incremental_state(spec, action_metadata)
            self.assertEqual(
                state,
                [
                    IncrementalStateItem(
                        source=Path("foo"),
                        destination_relative_to_bundle=Path("foo"),
                        digest="hash(foo)",
                        resolved_symlink=None,
                    ),
                    IncrementalStateItem(
                        source=Path("bar/baz"),
                        destination_relative_to_bundle=Path("tux/baz"),
                        digest="hash(baz)",
                        resolved_symlink=None,
                    ),
                    IncrementalStateItem(
                        source=Path("bar/qux"),
                        destination_relative_to_bundle=Path("tux/qux"),
                        digest=None,
                        resolved_symlink=Path("baz"),
                    ),
                    IncrementalStateItem(
                        source=Path("ghi/def"),
                        destination_relative_to_bundle=Path("ghi/def"),
                        digest="hash(def)",
                        resolved_symlink=None,
                    ),
                ],
            )

    def test_calculate_incremental_state_with_nested_directories(self) -> None:
        with tempfile.TemporaryDirectory() as project_root, chdir(project_root):
            # project_root
            #           └── res
            #                ├── b.txt
            #                ├── a.txt
            #                └── sub
            #                     ├── deep
            #                     │    └── d.txt
            #                     └── c.txt
            res_path = Path("res")
            (res_path / "sub" / "deep").mkdir(parents=True)
            (res_path / "b.txt").write_text("b")
            (res_path / "a.txt").write_text("a")
            (res_path / "sub" / "c.txt").write_text("c")
            (res_path / "sub" / "deep" / "d.txt").write_text("d")

            action_metadata = {
                "res/a.txt": "hash(a)",
                "res/b.txt": "hash(b)",
                "res/sub/c.txt": "hash(c)",
                "res/sub/deep/d.txt": "hash(d)",
            }
            spec = [
                BundleSpecItem(
                    src="res",
                    dst="Resources",
                    codesign_on_copy=False,
                )
            ]
            state = calculate_incremental_state(spec, action_metadata)
            self.assertEqual(
                state,
                [
                    IncrementalStateItem(
                        source=Path("res/a.txt"),
                        destination_relative_to_bundle=Path("Resources/a.txt"),
                        digest="hash(a)",
                        resolved_symlink=None,
                    ),
                    IncrementalStateItem(
                        source=Path("res/b.txt"),
                        destination_relative_to_bundle=Path("Resources/b.txt"),
                        digest="hash(b)",
                        resolved_symlink=None,
                    ),
                    IncrementalStateItem(
                        source=Path("res/sub/c.txt"),
                        destination_relative_to_bundle=Path("Resources/sub/c.txt"),
                        digest="hash(c)",
                        resolved_symlink=None,
                    ),
                    IncrementalStateItem(
                        source=Path("res/sub/deep/d.txt"),
                        destination_relative_to_bundle=Path("Resources/sub/deep/d.txt"),
                        digest="hash(d)",
                        resolved_symlink=None,
                    ),
                ],
            )

    def test_calculate_incremental_state_with_empty_destination(self) -> None:
        with tempfile.TemporaryDirectory() as project_root, chdir(project_root):
            res_path = Path("res")
            (res_path / "sub").mkdir(parents=True)
            (res_path / "a.txt").write_text("a")
            (res_path / "sub" / "b.txt").write_text("b")

            action_metadata = {
                "res/a.txt": "hash(a)",
                "res/sub/b.txt": "hash(b)",
            }
            spec = [
                BundleSpecItem(
                    src="res",
                    dst="",
                    codesign_on_copy=False,
                )
            ]
            state = calculate_incremental_state(spec, action_metadata)
            self.assertEqual(
                [item.destination_relative_to_bundle for item in state],
                [Path("a.txt"), Path("sub/b.txt")],
            )

    def test_calculate_incremental_state_prefers_action_metadata_to_resolving(
        self,
    ) -> None:
        with tempfile.TemporaryDirectory() as project_root, chdir(project_root):
            Path("foo").write_text("hello")
            action_metadata = {"foo": "hash(foo)"}
            spec = [BundleSpecItem(src="foo", dst="foo")]
            with patch.object(
                Path, "resolve", side_effect=AssertionError("path was resolved")
            ):
                state = calculate_incremental_state(spec, action_metadata)
            self.assertEqual(
                state,
                [
                    IncrementalStateItem(
                        source=Path("foo"),
                        destination_relative_to_bundle=Path("foo"),
                        digest="hash(foo)",
                        resolved_symlink=None,
                    ),
                ],
            )

    def test_calculate_incremental_state_with_ds_store(self) -> None:
        with tempfile.TemporaryDirectory() as project_root, chdir(project_root):
            # project_root
            #           ├── foo
            #           └── bar
            #                ├── baz
            #                └── .DS_Store
            Path("foo").write_text("hello")
            bar_path = Path("bar")
            bar_path.mkdir()
            (bar_path / "baz").write_text("world")
            (bar_path / ".DS_Store").touch()

            action_metadata = {
                "foo": "hash(foo)",
                "bar/baz": "hash(baz)",
            }
            spec = [
                BundleSpecItem(
                    src="foo",
                    dst="foo",
                    codesign_on_copy=False,
                ),
                BundleSpecItem(
                    src="bar",
                    dst="bar",
                    codesign_on_copy=True,
                ),
            ]
            state = calculate_incremental_state(spec, action_metadata)
            self.assertEqual(
                state,
                [
                    IncrementalStateItem(
                        source=Path("foo"),
                        destination_relative_to_bundle=Path("foo"),
                        digest="hash(foo)",
                        resolved_symlink=None,
                    ),
                    IncrementalStateItem(
                        source=Path("bar/baz"),
                        destination_relative_to_bundle=Path("bar/baz"),
                        digest="hash(baz)",
                        resolved_symlink=None,
                    ),
                ],
            )

    def test_codesign_on_copy_entitlements_use_the_action_metadata_key(self) -> None:
        # The entitlements digest is looked up in whatever `parse_action_metadata`
        # returned, while the spec supplies the entitlements path as a `Path`. If
        # the two disagree on the key, every entitlements lookup misses and the
        # bundling action fails outright, so pin them together here.
        metadata = parse_action_metadata(
            io.StringIO(
                '{"version": 1, "digests": [{"path": "baz/entitlements.plist", '
                '"digest": "entitlements_digest"}]}'
            )
        )
        self.assertIsNotNone(metadata)
        incremental_context = IncrementalContext(
            metadata=metadata,
            state=None,
            codesigned=True,
            codesign_configuration=None,
            codesign_identity="identity",
            codesign_arguments=[],
            versioned_if_macos=True,
        )
        self.assertEqual(
            codesigned_on_copy_item(
                path=Path("foo"),
                entitlements=Path("baz/entitlements.plist"),
                incremental_context=incremental_context,
                codesign_flags_override=None,
                extra_codesign_paths=None,
            ),
            CodesignedOnCopy(
                path=Path("foo"),
                entitlements_digest="entitlements_digest",
                codesign_flags_override=None,
                extra_codesign_paths=None,
            ),
        )
