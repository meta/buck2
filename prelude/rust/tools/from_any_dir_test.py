import os
import runpy
import unittest
from pathlib import Path


class FromAnyDirTest(unittest.TestCase):
    def test_relative_interim_cwd_works_on_python_3_11(self) -> None:
        namespace = runpy.run_path(
            os.path.join(os.path.dirname(__file__), "from_any_dir.py")
        )
        relative_interim_cwd = namespace["relative_interim_cwd"]

        self.assertEqual(
            relative_interim_cwd(Path("/workspace/project"), Path("/workspace")),
            Path(".."),
        )


if __name__ == "__main__":
    unittest.main()
