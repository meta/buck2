#!/usr/bin/env fbpython
# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

import pathlib
import tempfile
import unittest
import zipfile

from fat_jar import _prepare_meta_inf_override


class PrepareMetaInfOverrideTest(unittest.TestCase):
    def test_without_concat_jars_points_to_directory(self):
        # Setup: create a meta_inf_staging directory with a META-INF/services file.
        with tempfile.TemporaryDirectory() as temp_dir:
            meta_inf_staging = pathlib.Path(temp_dir) / "meta_inf_staging"
            meta_inf_staging.mkdir()
            services_dir = meta_inf_staging / "META-INF" / "services"
            services_dir.mkdir(parents=True)
            service_file = services_dir / "com.example.MyService"
            service_file.write_text("com.example.MyServiceImpl\n")

            override_dir = pathlib.Path(temp_dir) / "override"
            override_dir.mkdir()

            # Execute: call with concat_jars=False.
            result = _prepare_meta_inf_override(
                meta_inf_staging, str(override_dir), concat_jars=False
            )

            # Assert: the returned file points to the staging directory (not a jar).
            self.assertTrue(result.exists())
            override_path = result.read_text()
            self.assertEqual(override_path, str(meta_inf_staging))
            self.assertTrue(pathlib.Path(override_path).is_dir())

    def test_with_concat_jars_points_to_jar(self):
        # Setup: create a meta_inf_staging directory with a META-INF/services file.
        with tempfile.TemporaryDirectory() as temp_dir:
            meta_inf_staging = pathlib.Path(temp_dir) / "meta_inf_staging"
            meta_inf_staging.mkdir()
            services_dir = meta_inf_staging / "META-INF" / "services"
            services_dir.mkdir(parents=True)
            service_file = services_dir / "com.example.MyService"
            service_file.write_text("com.example.MyServiceImpl\n")

            override_dir = pathlib.Path(temp_dir) / "override"
            override_dir.mkdir()

            # Execute: call with concat_jars=True.
            result = _prepare_meta_inf_override(
                meta_inf_staging, str(override_dir), concat_jars=True
            )

            # Assert: the returned file points to a valid jar file (not a directory).
            self.assertTrue(result.exists())
            jar_path = pathlib.Path(result.read_text())
            self.assertTrue(jar_path.exists())
            self.assertTrue(str(jar_path).endswith(".jar"))

            # Verify the jar contains the expected META-INF entry.
            with zipfile.ZipFile(jar_path, "r") as zf:
                names = zf.namelist()
                self.assertIn("META-INF/services/com.example.MyService", names)
                content = zf.read("META-INF/services/com.example.MyService").decode()
                self.assertEqual(content, "com.example.MyServiceImpl\n")

    def test_with_concat_jars_multiple_files(self):
        # Setup: create a meta_inf_staging directory with multiple META-INF files.
        with tempfile.TemporaryDirectory() as temp_dir:
            meta_inf_staging = pathlib.Path(temp_dir) / "meta_inf_staging"
            meta_inf_staging.mkdir()
            services_dir = meta_inf_staging / "META-INF" / "services"
            services_dir.mkdir(parents=True)
            (services_dir / "com.example.ServiceA").write_text("ImplA\n")
            (services_dir / "com.example.ServiceB").write_text("ImplB\n")

            override_dir = pathlib.Path(temp_dir) / "override"
            override_dir.mkdir()

            # Execute: call with concat_jars=True.
            result = _prepare_meta_inf_override(
                meta_inf_staging, str(override_dir), concat_jars=True
            )

            # Assert: the jar contains both service files.
            jar_path = pathlib.Path(result.read_text())
            with zipfile.ZipFile(jar_path, "r") as zf:
                names = zf.namelist()
                self.assertIn("META-INF/services/com.example.ServiceA", names)
                self.assertIn("META-INF/services/com.example.ServiceB", names)

    def test_with_concat_jars_skips_directories(self):
        # Setup: create a meta_inf_staging directory with nested subdirectories.
        with tempfile.TemporaryDirectory() as temp_dir:
            meta_inf_staging = pathlib.Path(temp_dir) / "meta_inf_staging"
            meta_inf_staging.mkdir()
            nested_dir = meta_inf_staging / "META-INF" / "nested" / "deep"
            nested_dir.mkdir(parents=True)
            (nested_dir / "file.txt").write_text("content\n")

            override_dir = pathlib.Path(temp_dir) / "override"
            override_dir.mkdir()

            # Execute: call with concat_jars=True.
            result = _prepare_meta_inf_override(
                meta_inf_staging, str(override_dir), concat_jars=True
            )

            # Assert: only the file is in the jar, not directory entries.
            jar_path = pathlib.Path(result.read_text())
            with zipfile.ZipFile(jar_path, "r") as zf:
                names = zf.namelist()
                self.assertEqual(names, ["META-INF/nested/deep/file.txt"])
