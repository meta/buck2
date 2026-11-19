# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

import os
import stat
import tempfile
import unittest
import zipfile

import unzip


def write_archive(path, entries):
    """`entries` holds `(name, content)` files and `(name, target, "symlink")` symlinks."""
    with zipfile.ZipFile(path, "w") as archive:
        for entry in entries:
            if len(entry) == 2:
                archive.writestr(entry[0], entry[1])
            else:
                info = zipfile.ZipInfo(entry[0])
                info.external_attr = (stat.S_IFLNK | 0o777) << 16
                archive.writestr(info, entry[1])


class UnzipTest(unittest.TestCase):
    def test_a_symlink_to_a_file_through_a_directory_link_is_extracted(self):
        with tempfile.TemporaryDirectory() as tmp_dir:
            archive = os.path.join(tmp_dir, "a.zip")
            out = os.path.join(tmp_dir, "out")
            write_archive(
                archive,
                [("a/f", "inside"), ("s", "a", "symlink"), ("t", "s/f", "symlink")],
            )
            unzip.do_unzip(archive, out)
            with open(os.path.join(out, "t")) as linked:
                self.assertEqual(linked.read(), "inside")

    def test_a_symlink_chain_that_leaves_the_output_directory_is_rejected(self):
        with tempfile.TemporaryDirectory() as tmp_dir:
            archive = os.path.join(tmp_dir, "a.zip")
            out = os.path.join(tmp_dir, "out")
            write_archive(
                archive,
                [("a/f", "inside"), ("s", ".", "symlink"), ("t", "s/..", "symlink")],
            )
            with self.assertRaisesRegex(RuntimeError, "`t`.*outside"):
                unzip.do_unzip(archive, out)

    def test_an_absolute_symlink_name_is_rejected(self):
        with tempfile.TemporaryDirectory() as tmp_dir:
            archive = os.path.join(tmp_dir, "a.zip")
            out = os.path.join(tmp_dir, "out")
            outside = os.path.join(tmp_dir, "escaped_link")
            write_archive(archive, [("a/f", "inside"), (outside, "a", "symlink")])
            with self.assertRaisesRegex(RuntimeError, "outside"):
                unzip.do_unzip(archive, out)
            self.assertFalse(os.path.lexists(outside))

    def test_a_parent_directory_symlink_name_is_rejected(self):
        with tempfile.TemporaryDirectory() as tmp_dir:
            archive = os.path.join(tmp_dir, "a.zip")
            out = os.path.join(tmp_dir, "out")
            write_archive(
                archive, [("a/f", "inside"), ("../escaped_link", "a", "symlink")]
            )
            with self.assertRaisesRegex(RuntimeError, "outside"):
                unzip.do_unzip(archive, out)
            self.assertFalse(os.path.lexists(os.path.join(tmp_dir, "escaped_link")))
