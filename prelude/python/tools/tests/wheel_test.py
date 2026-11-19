# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.


import csv
import io
import tempfile
import unittest
import zipfile
from pathlib import Path

import wheel


class WheelBuilderTest(unittest.TestCase):
    def test_readme_sets_metadata_body_and_content_type(self) -> None:
        with tempfile.TemporaryDirectory() as tmpdir:
            tmp = Path(tmpdir)
            output = tmp / "demo-1.0-py3-none-any.whl"
            readme = tmp / "README.md"
            module = tmp / "demo.py"

            readme.write_text("# Demo\n\nLong description.\n", encoding="utf-8")
            module.write_text("print('ok')\n", encoding="utf-8")

            with wheel.WheelBuilder(
                name="demo",
                version="1.0",
                output=str(output),
                readme=str(readme),
            ) as whl:
                whl.write("demo.py", str(module))

            with zipfile.ZipFile(output) as archive:
                metadata = archive.read("demo-1.0.dist-info/METADATA").decode("utf-8")

            self.assertIn("Description-Content-Type: text/markdown\n", metadata)
            self.assertTrue(metadata.endswith("\n# Demo\n\nLong description.\n"))

    def test_record_round_trips_paths_containing_commas(self) -> None:
        # Resource filenames with commas are real: vLLM ships fused-MoE configs
        # named like `E=128,N=768,device_name=NVIDIA_H100.json`.
        comma_path = "demo/E=128,N=768,device_name=X.json"

        with tempfile.TemporaryDirectory() as tmpdir:
            tmp = Path(tmpdir)
            output = tmp / "demo-1.0-py3-none-any.whl"
            plain = tmp / "demo.py"
            config = tmp / "config.json"

            plain.write_text("print('ok')\n", encoding="utf-8")
            config.write_text("{}\n", encoding="utf-8")

            with wheel.WheelBuilder(
                name="demo", version="1.0", output=str(output)
            ) as whl:
                whl.write("demo.py", str(plain))
                whl.write(comma_path, str(config))

            with zipfile.ZipFile(output) as archive:
                record = archive.read("demo-1.0.dist-info/RECORD").decode("utf-8")

            rows = [row for row in csv.reader(io.StringIO(record)) if row]
            self.assertTrue(all(len(row) == 3 for row in rows), record)
            paths = [row[0] for row in rows]
            self.assertIn(comma_path, paths)
            # Paths needing no quoting stay byte-identical to the plain form,
            # so this change is inert for wheels that have no such filenames.
            self.assertIn("demo.py,,\n", record)
