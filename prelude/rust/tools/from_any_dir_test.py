import os
import runpy
import unittest
from unittest import mock


class FromAnyDirTest(unittest.TestCase):
    def test_executes_bare_command_through_path(self) -> None:
        namespace = runpy.run_path(os.path.join(os.path.dirname(__file__), "from_any_dir.py"))
        with mock.patch.object(namespace["os"], "execvp") as execvp:
            namespace["exec_command"](["clang", "--version"])

        execvp.assert_called_once_with("clang", ["clang", "--version"])


if __name__ == "__main__":
    unittest.main()
