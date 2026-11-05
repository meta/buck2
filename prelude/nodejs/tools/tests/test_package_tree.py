# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

from __future__ import annotations

import contextlib
import errno
import io
import json
import os
import stat
import sys
import tempfile
import unittest
from pathlib import Path
from unittest import mock

from package_tree import (
    assemble_package,
    main,
    manifest,
    run_build,
    strip_types,
    validate_package,
    write_validation_result,
)


def _write(path: Path, text: str = "") -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(text)


def _files(root: Path) -> list[str]:
    return sorted(
        p.relative_to(root).as_posix() for p in root.rglob("*") if p.is_file()
    )


class TestAssemblePackage(unittest.TestCase):
    def setUp(self) -> None:
        tmp = tempfile.TemporaryDirectory()
        self.addCleanup(tmp.cleanup)
        self.root = Path(tmp.name)
        self.emit = self.root / "emit"
        self.out = self.root / "out"
        _write(self.emit / "cmd" / "render.js", "js")
        _write(self.emit / "cmd" / "render.d.ts", "dts")
        _write(self.root / "package.json", "supplied")

    def _assemble(
        self, package_subdir: str, *entries: tuple[str, str], package_json: bool = False
    ) -> list[str]:
        return assemble_package(
            self.emit,
            package_subdir,
            [(path, self.root / src, f"entry '{src}'") for path, src in entries],
            self.root / "package.json" if package_json else None,
            self.out,
        )

    def assertErrorsMention(
        self, errors: list[str], *expected: tuple[str, ...]
    ) -> None:
        self.assertEqual(len(errors), len(expected), errors)
        for error, parts in zip(errors, expected):
            for part in parts:
                self.assertIn(part, error)

    def test_emit_under_package_subdir_beside_package_json(self) -> None:
        self.assertEqual(self._assemble("lib/js", package_json=True), [])
        self.assertEqual(
            _files(self.out),
            ["lib/js/cmd/render.d.ts", "lib/js/cmd/render.js", "package.json"],
        )
        self.assertEqual((self.out / "lib/js/cmd/render.js").read_text(), "js")
        self.assertEqual((self.out / "package.json").read_text(), "supplied")

    def test_emit_at_root(self) -> None:
        self.assertEqual(self._assemble(""), [])
        self.assertEqual(_files(self.out), ["cmd/render.d.ts", "cmd/render.js"])

    def test_package_json_replaces_emitted_manifest(self) -> None:
        _write(self.emit / "package.json", "emitted")
        os.chmod(self.emit / "package.json", 0o444)
        self.assertEqual(self._assemble("", package_json=True), [])
        self.assertEqual((self.out / "package.json").read_text(), "supplied")

    def test_package_json_on_emitted_directory_collides(self) -> None:
        _write(self.emit / "package.json" / "index.js", "js")
        self.assertErrorsMention(
            self._assemble("", package_json=True),
            ("`package_json`", "collides", "directory 'package.json'"),
        )
        self.assertEqual((self.out / "package.json" / "index.js").read_text(), "js")

    def test_package_json_that_cannot_be_copied_is_reported(self) -> None:
        (self.root / "package.json").unlink()
        self.assertErrorsMention(
            self._assemble("", package_json=True),
            ("`package_json`", "'package.json'", os.strerror(errno.ENOENT)),
        )
        self.assertEqual(_files(self.out), ["cmd/render.d.ts", "cmd/render.js"])

    def test_entries_merge_into_emitted_directories(self) -> None:
        _write(self.root / "data.json", "data")
        _write(self.root / "templates" / "a.txt", "a")
        _write(self.root / "templates" / "nested" / "b.txt", "b")
        _write(self.root / "assets" / "render.css", "css")
        self.assertEqual(
            self._assemble(
                "dist",
                ("dist/cmd/data.json", "data.json"),
                ("dist/templates", "templates"),
                ("dist/cmd", "assets"),
            ),
            [],
        )
        self.assertEqual(
            _files(self.out),
            [
                "dist/cmd/data.json",
                "dist/cmd/render.css",
                "dist/cmd/render.d.ts",
                "dist/cmd/render.js",
                "dist/templates/a.txt",
                "dist/templates/nested/b.txt",
            ],
        )
        self.assertEqual((self.out / "dist/templates/nested/b.txt").read_text(), "b")

    def test_collision_names_the_entry_and_the_emitted_path(self) -> None:
        _write(self.root / "file.txt")
        _write(self.root / "dir" / "render.js")
        _write(self.root / "dir" / "render.css", "css")
        for i, (path, src, message) in enumerate(
            [
                ("dist/cmd/render.js", "file.txt", "file 'dist/cmd/render.js'"),
                ("dist/cmd", "file.txt", "directory 'dist/cmd'"),
                (
                    "dist/cmd/render.d.ts/x.txt",
                    "file.txt",
                    "file 'dist/cmd/render.d.ts'",
                ),
                ("dist/cmd/render.js", "dir", "file 'dist/cmd/render.js'"),
                ("dist/cmd", "dir", "file 'dist/cmd/render.js'"),
            ]
        ):
            with self.subTest(path=path, src=src):
                self.out = self.root / f"out{i}"
                self.assertErrorsMention(
                    self._assemble("dist", (path, src)),
                    (f"entry '{src}'", "collides", message),
                )
                self.assertEqual((self.out / "dist/cmd/render.js").read_text(), "js")
        self.assertEqual((self.root / "out4/dist/cmd/render.css").read_text(), "css")

    def test_every_collision_reported(self) -> None:
        _write(self.root / "a.txt")
        _write(self.root / "dir" / "render.js")
        _write(self.root / "dir" / "render.d.ts")
        _write(self.root / "dir" / "extra.txt")
        self.assertErrorsMention(
            self._assemble(
                "",
                ("cmd/render.d.ts", "a.txt"),
                ("cmd/render.js", "a.txt"),
                ("ok.txt", "a.txt"),
            ),
            ("entry 'a.txt'", "collides", "file 'cmd/render.d.ts'"),
            ("entry 'a.txt'", "collides", "file 'cmd/render.js'"),
        )
        self.assertTrue((self.out / "ok.txt").exists())
        self.out = self.root / "out2"
        self.assertErrorsMention(
            self._assemble("", ("cmd", "dir")),
            ("entry 'dir'", "collides", "file 'cmd/render.d.ts'"),
            ("entry 'dir'", "collides", "file 'cmd/render.js'"),
        )
        self.assertTrue((self.out / "cmd" / "extra.txt").exists())

    def test_cli_rejects_package_subdir_with_prebuilt(self) -> None:
        entries = self.root / "entries.json"
        entries.write_text("[]")
        argv = [
            "package_tree.py",
            "assemble-package",
            f"--prebuilt={self.emit}",
            "--package-subdir=sub",
            f"--entries={entries}",
            f"--out={self.out}",
        ]
        stderr = io.StringIO()
        with (
            mock.patch.object(sys, "argv", argv),
            contextlib.redirect_stderr(stderr),
        ):
            self.assertEqual(main(), 1)
        self.assertEqual(
            stderr.getvalue(),
            "assemble-package: --package-subdir cannot be combined with --prebuilt\n",
        )
        self.assertFalse(self.out.exists())

    def test_prebuilt_collisions_name_the_prebuilt_file(self) -> None:
        _write(self.root / "file.txt")
        _write(self.emit / "package.json" / "index.js", "js")
        self.assertEqual(
            assemble_package(
                self.emit,
                "",
                [
                    ("cmd/render.js", self.root / "file.txt", "entry 'file.txt'"),
                    ("cmd/render.d.ts/x.txt", self.root / "file.txt", "entry 'x'"),
                ],
                self.root / "package.json",
                self.out,
                origin="prebuilt",
            ),
            [
                "entry 'file.txt' collides with the prebuilt file 'cmd/render.js'",
                "entry 'x' collides with the prebuilt file 'cmd/render.d.ts'",
                "`package_json` collides with the prebuilt directory 'package.json'",
            ],
        )

    @unittest.skipIf(sys.platform == "win32", "Windows has no executable bit")
    def test_executable_bit_kept(self) -> None:
        _write(self.root / "bin" / "run.sh", "#!/bin/sh")
        os.chmod(self.root / "bin" / "run.sh", 0o755)
        self.assertEqual(self._assemble("dist", ("bin", "bin")), [])
        mode = stat.S_IMODE(os.stat(self.out / "bin" / "run.sh").st_mode)
        self.assertEqual(mode & 0o111, 0o111)

    @unittest.skipIf(sys.platform == "win32", "creating symlinks needs privileges")
    def test_symlinks_copied_as_targets(self) -> None:
        _write(self.root / "store" / "a.txt", "a")
        _write(self.root / "res" / "b.txt", "b")
        os.symlink("../store", self.root / "res" / "linked", target_is_directory=True)
        os.symlink("b.txt", self.root / "res" / "c.txt")
        os.symlink("../store", self.emit / "linked", target_is_directory=True)
        os.symlink("render.js", self.emit / "cmd" / "alias.js")
        self.assertEqual(self._assemble("dist", ("dist/res", "res")), [])
        self.assertEqual(
            _files(self.out),
            [
                "dist/cmd/alias.js",
                "dist/cmd/render.d.ts",
                "dist/cmd/render.js",
                "dist/linked/a.txt",
                "dist/res/b.txt",
                "dist/res/c.txt",
                "dist/res/linked/a.txt",
            ],
        )
        for path in _files(self.out) + ["dist/linked", "dist/res/linked"]:
            self.assertFalse((self.out / path).is_symlink(), path)
        self.assertEqual((self.out / "dist/res/c.txt").read_text(), "b")
        self.assertEqual((self.out / "dist/cmd/alias.js").read_text(), "js")

    @unittest.skipIf(sys.platform == "win32", "creating symlinks needs privileges")
    def test_symlink_cycle_in_emit_fails(self) -> None:
        for target in (".", ".."):
            with self.subTest(target=target):
                self.out = self.root / f"out{len(target)}"
                loop = self.emit / "cmd" / "loop"
                loop.unlink(missing_ok=True)
                os.symlink(target, loop, target_is_directory=True)
                with self.assertRaisesRegex(ValueError, "symlink cycle: .*loop"):
                    self._assemble("dist")

    @unittest.skipIf(sys.platform == "win32", "creating symlinks needs privileges")
    def test_symlink_cycle_in_entry_reported_with_other_errors(self) -> None:
        _write(self.root / "a.txt")
        _write(self.root / "res" / "sub" / "b.txt", "b")
        for target in (".", ".."):
            with self.subTest(target=target):
                self.out = self.root / f"out{len(target)}"
                loop = self.root / "res" / "sub" / "loop"
                loop.unlink(missing_ok=True)
                os.symlink(target, loop, target_is_directory=True)
                errors = self._assemble("", ("cmd/render.js", "a.txt"), ("res", "res"))
                self.assertEqual(len(errors), 2, errors)
                self.assertEqual(
                    errors[0],
                    "entry 'a.txt' collides with the emitted file 'cmd/render.js'",
                )
                self.assertRegex(
                    errors[1],
                    r"^entry 'res': cannot copy 'res/sub/loop(/\w+)*': symlink cycle: .*loop",
                )
                self.assertEqual((self.out / "res/sub/b.txt").read_text(), "b")

    @unittest.skipIf(sys.platform == "win32", "creating symlinks needs privileges")
    def test_file_that_cannot_be_copied_names_the_entry(self) -> None:
        _write(self.root / "res" / "a.txt", "a")
        os.symlink("missing.txt", self.root / "res" / "dangling.txt")
        self.assertErrorsMention(
            self._assemble("dist", ("dist/res", "res")),
            ("entry 'res'", "'dist/res/dangling.txt'", os.strerror(errno.ENOENT)),
        )
        self.assertEqual((self.out / "dist/res/a.txt").read_text(), "a")

    @unittest.skipIf(
        sys.platform == "win32" or os.geteuid() == 0,
        "needs POSIX permissions that apply to the current user",
    )
    def test_directory_that_cannot_be_read_names_the_entry(self) -> None:
        _write(self.root / "res" / "a.txt", "a")
        _write(self.root / "res" / "locked" / "b.txt", "b")
        os.chmod(self.root / "res" / "locked", 0)
        self.addCleanup(os.chmod, self.root / "res" / "locked", 0o755)
        self.assertErrorsMention(
            self._assemble("dist", ("dist/res", "res")),
            ("entry 'res'", "'dist/res/locked'", os.strerror(errno.EACCES)),
        )
        self.assertEqual((self.out / "dist/res/a.txt").read_text(), "a")

    def test_parent_directory_that_cannot_be_created_names_the_entry(self) -> None:
        _write(self.root / "a.txt", "a")
        _write(self.root / "b.txt", "b")
        mkdir = Path.mkdir

        def failing_mkdir(path: Path, *args: object, **kwargs: object) -> None:
            if path == self.out / "locked":
                raise PermissionError(errno.EACCES, os.strerror(errno.EACCES))
            mkdir(path, *args, **kwargs)

        with mock.patch.object(Path, "mkdir", autospec=True, side_effect=failing_mkdir):
            errors = self._assemble(
                "dist", ("locked/a.txt", "a.txt"), ("b.txt", "b.txt")
            )
        self.assertErrorsMention(
            errors, ("entry 'a.txt'", "'locked/a.txt'", os.strerror(errno.EACCES))
        )
        self.assertEqual((self.out / "b.txt").read_text(), "b")

    def test_cli_reports_missing_package_json(self) -> None:
        _write(self.root / "entries.json", "[]")
        argv = ["package_tree.py", "assemble-package", "--emit", str(self.emit)]
        argv += ["--entries", str(self.root / "entries.json")]
        argv += ["--package-json", str(self.root / "missing.json")]
        stderr = io.StringIO()
        with (
            mock.patch.object(sys, "argv", argv + ["--out", str(self.root / "out")]),
            contextlib.redirect_stderr(stderr),
        ):
            self.assertEqual(main(), 1)
        self.assertEqual(
            stderr.getvalue(),
            "`package_json`: cannot copy 'package.json': "
            f"{os.strerror(errno.ENOENT)}\n",
        )


class TestRunBuild(unittest.TestCase):
    def setUp(self) -> None:
        tmp = tempfile.TemporaryDirectory()
        self.addCleanup(tmp.cleanup)
        self.tmp = Path(tmp.name)
        self.staged = self.tmp / "staged"
        _write(self.staged / "input.txt", "input")
        _write(self.staged / "src" / "nested.txt", "nested")
        _write(self.staged / "node_modules" / "tool" / "index.js", "tool")
        self.out = self.tmp / "out"

    def _run(self, script: str, root_prefix: str = "") -> int:
        return run_build(
            self.staged,
            "dist",
            self.out,
            [sys.executable, "-c", script],
            self.tmp / "scratch",
            root_prefix,
        )

    def test_copies_the_output_directory_and_keeps_the_staged_one(self) -> None:
        script = (
            "import os, pathlib\n"
            "pathlib.Path('node_modules/.cache').mkdir()\n"
            "pathlib.Path('dist/sub').mkdir(parents=True)\n"
            "text = open('input.txt').read() + open('src/nested.txt').read()\n"
            "text += open('node_modules/tool/index.js').read()\n"
            "pathlib.Path('dist/sub/out.txt').write_text(text)\n"
            "open('input.txt', 'w').write('changed')\n"
        )
        self.assertEqual(self._run(script), 0)
        self.assertEqual(_files(self.out), ["sub/out.txt"])
        self.assertEqual((self.out / "sub" / "out.txt").read_text(), "inputnestedtool")
        self.assertEqual(
            _files(self.staged),
            ["input.txt", "node_modules/tool/index.js", "src/nested.txt"],
        )
        self.assertEqual((self.staged / "input.txt").read_text(), "input")

    def test_replaces_the_root_prefix_with_the_current_directory(self) -> None:
        script = (
            "import pathlib, sys\n"
            "pathlib.Path('dist').mkdir()\n"
            "pathlib.Path('dist/arg.txt').write_text(sys.argv[1])\n"
        )
        code = run_build(
            self.staged,
            "dist",
            self.out,
            [sys.executable, "-c", script, "ROOT/some/file"],
            self.tmp / "scratch",
            "ROOT/",
        )
        self.assertEqual(code, 0)
        self.assertEqual(
            (self.out / "arg.txt").read_text(),
            os.path.join(os.getcwd(), "some/file"),
        )

    def test_replaces_the_root_prefix_in_env_keys(self) -> None:
        script = (
            "import os, pathlib\n"
            "pathlib.Path('dist').mkdir()\n"
            "pathlib.Path('dist/env.txt').write_text(os.environ['BUILD_INPUT'])\n"
        )
        with mock.patch.dict(os.environ, {"BUILD_INPUT": "ROOT/some/file"}):
            code = run_build(
                self.staged,
                "dist",
                self.out,
                [sys.executable, "-c", script],
                self.tmp / "scratch",
                "ROOT/",
                ["BUILD_INPUT"],
            )
        self.assertEqual(code, 0)
        self.assertEqual(
            (self.out / "env.txt").read_text(),
            os.path.join(os.getcwd(), "some/file"),
        )

    def test_copies_symlinks_in_the_output_as_their_targets(self) -> None:
        script = (
            "import os, pathlib\n"
            "pathlib.Path('dist').mkdir()\n"
            "os.symlink('../node_modules/tool', 'dist/tool')\n"
            "os.symlink('../input.txt', 'dist/input.txt')\n"
        )
        self.assertEqual(self._run(script), 0)
        self.assertEqual(_files(self.out), ["input.txt", "tool/index.js"])
        self.assertFalse((self.out / "tool").is_symlink())
        self.assertEqual((self.out / "tool" / "index.js").read_text(), "tool")

    def test_dangling_symlink_in_the_output_raises(self) -> None:
        script = (
            "import os, pathlib\n"
            "pathlib.Path('dist').mkdir()\n"
            "os.symlink('missing', 'dist/link')\n"
        )
        with self.assertRaises(FileNotFoundError):
            self._run(script)

    def test_command_failure_is_returned(self) -> None:
        self.assertEqual(self._run("import sys; sys.exit(3)"), 3)
        self.assertFalse(self.out.exists())

    def test_missing_output_directory_fails(self) -> None:
        stderr = io.StringIO()
        with contextlib.redirect_stderr(stderr):
            self.assertEqual(self._run("pass"), 1)
        self.assertIn("the command wrote no directory at 'dist'", stderr.getvalue())


class TestManifest(unittest.TestCase):
    def setUp(self) -> None:
        tmp = tempfile.TemporaryDirectory()
        self.addCleanup(tmp.cleanup)
        self.root = Path(tmp.name)
        self.tree = self.root / "node_modules"

    def test_lists_packages_and_scoped_packages(self) -> None:
        _write(self.tree / "a" / "package.json", json.dumps({"version": "1.0.0"}))
        _write(
            self.tree / "@scope" / "x" / "package.json",
            json.dumps({"version": "2.0.0"}),
        )
        _write(self.tree / "@scope" / "y" / "index.js")
        _write(self.tree / "nometa" / "index.js")
        _write(self.tree / "stray.txt")
        self.assertEqual(
            manifest(self.tree),
            {
                "packages": [
                    {"name": "@scope/x", "version": "2.0.0"},
                    {"name": "@scope/y"},
                    {"name": "a", "version": "1.0.0"},
                    {"name": "nometa"},
                    {"name": "stray.txt", "file": True},
                ],
                "ignored": [],
            },
        )

    def test_records_platform_fields_and_bins(self) -> None:
        _write(
            self.tree / "native" / "package.json",
            json.dumps(
                {
                    "version": "1.0.0",
                    "os": ["linux", "!win32"],
                    "cpu": ["x64"],
                    "libc": ["glibc"],
                    "bin": "cli.js",
                }
            ),
        )
        _write(
            self.tree / "@scope" / "tool" / "package.json",
            json.dumps({"bin": {"tool": "bin/tool.js", "t": "bin/t.js"}}),
        )
        self.assertEqual(
            manifest(self.tree)["packages"],
            [
                {
                    "name": "@scope/tool",
                    "bin": {"tool": "bin/tool.js", "t": "bin/t.js"},
                },
                {
                    "name": "native",
                    "version": "1.0.0",
                    "os": ["linux", "!win32"],
                    "cpu": ["x64"],
                    "libc": ["glibc"],
                    "bin": {"native": "cli.js"},
                },
            ],
        )

    def test_dependency_names_include_optional_and_peer(self) -> None:
        _write(
            self.tree / "a" / "package.json",
            json.dumps(
                {
                    "dependencies": {"b": "^1", "@scope/c": "^2"},
                    "optionalDependencies": {"fsevents": "^2"},
                    "peerDependencies": {"react": "*", "b": "^1"},
                    "devDependencies": {"jest": "*"},
                }
            ),
        )
        self.assertEqual(
            manifest(self.tree)["packages"],
            [{"name": "a", "dependencies": ["@scope/c", "b", "fsevents", "react"]}],
        )

    def test_nested_node_modules_belong_to_their_package(self) -> None:
        _write(self.tree / "a" / "package.json", json.dumps({"version": "1.0.0"}))
        _write(
            self.tree / "a" / "node_modules" / "b" / "package.json",
            json.dumps({"version": "9.9.9"}),
        )
        _write(self.tree / "a" / "node_modules" / "@scope" / "c" / "index.js")
        self.assertEqual(
            manifest(self.tree)["packages"], [{"name": "a", "version": "1.0.0"}]
        )

    def test_dot_entries_are_ignored(self) -> None:
        _write(self.tree / ".yarn-integrity")
        _write(self.tree / ".bin" / "cli")
        _write(self.tree / ".package-lock.json")
        _write(self.tree / "a" / "index.js")
        self.assertEqual(
            manifest(self.tree),
            {
                "packages": [{"name": "a"}],
                "ignored": [".bin", ".package-lock.json", ".yarn-integrity"],
            },
        )

    def test_dot_entries_in_a_scope_are_ignored(self) -> None:
        _write(self.tree / "@scope" / ".cache" / "x")
        _write(self.tree / "@scope" / "a" / "index.js")
        self.assertEqual(
            manifest(self.tree),
            {"packages": [{"name": "@scope/a"}], "ignored": ["@scope/.cache"]},
        )

    def test_malformed_package_json_records_an_error(self) -> None:
        _write(self.tree / "bad" / "package.json", "{not json")
        _write(self.tree / "good" / "package.json", json.dumps({"version": "1.0.0"}))
        bad, good = manifest(self.tree)["packages"]
        self.assertEqual(set(bad), {"name", "error"})
        self.assertEqual(bad["name"], "bad")
        self.assertRegex(bad["error"], r"cannot read '.*package\.json'")
        self.assertEqual(good, {"name": "good", "version": "1.0.0"})

    def test_package_json_that_is_not_an_object_records_an_error(self) -> None:
        _write(self.tree / "list" / "package.json", "[]")
        [package] = manifest(self.tree)["packages"]
        self.assertEqual(set(package), {"name", "error"})
        self.assertRegex(package["error"], "is not a JSON object")

    def test_unreadable_package_json_records_an_error(self) -> None:
        (self.tree / "dir" / "package.json").mkdir(parents=True)
        [package] = manifest(self.tree)["packages"]
        self.assertEqual(set(package), {"name", "error"})
        self.assertRegex(package["error"], "cannot read")

    def test_unreadable_scoped_package_leaves_its_siblings_described(self) -> None:
        (self.tree / "@scope" / "bad" / "package.json").mkdir(parents=True)
        _write(
            self.tree / "@scope" / "good" / "package.json",
            json.dumps({"version": "1.0.0"}),
        )
        bad, good = manifest(self.tree)["packages"]
        self.assertEqual(set(bad), {"name", "error"})
        self.assertEqual(bad["name"], "@scope/bad")
        self.assertRegex(bad["error"], "cannot read")
        self.assertEqual(good, {"name": "@scope/good", "version": "1.0.0"})

    @unittest.skipIf(
        sys.platform == "win32" or os.geteuid() == 0,
        "needs POSIX permissions that apply to the current user",
    )
    def test_scope_that_cannot_be_listed_records_an_error(self) -> None:
        _write(self.tree / "@locked" / "a" / "package.json", "{}")
        _write(self.tree / "good" / "package.json", json.dumps({"version": "1.0.0"}))
        os.chmod(self.tree / "@locked", 0)
        self.addCleanup(os.chmod, self.tree / "@locked", 0o755)
        locked, good = manifest(self.tree)["packages"]
        self.assertEqual(set(locked), {"name", "error"})
        self.assertEqual(locked["name"], "@locked")
        self.assertRegex(locked["error"], "Permission denied")
        self.assertEqual(good, {"name": "good", "version": "1.0.0"})

    def test_mistyped_fields_are_left_out(self) -> None:
        _write(
            self.tree / "odd" / "package.json",
            json.dumps({"bin": 3, "dependencies": ["x"]}),
        )
        self.assertEqual(manifest(self.tree)["packages"], [{"name": "odd"}])

    @unittest.skipIf(sys.platform == "win32", "creating symlinks needs privileges")
    def test_links_inside_the_tree_record_their_path(self) -> None:
        _write(self.tree / "real" / "package.json", json.dumps({"version": "1.0.0"}))
        _write(
            self.tree / "store" / "x" / "package.json", json.dumps({"version": "2.0.0"})
        )
        os.symlink("real", self.tree / "alias", target_is_directory=True)
        _write(self.tree / "store" / "README")
        os.symlink("store", self.tree / "@linked", target_is_directory=True)
        (self.tree / "@scope").mkdir()
        os.symlink("../real", self.tree / "@scope" / "y", target_is_directory=True)
        os.symlink("README", self.tree / "store" / "readme_link")
        os.symlink("store/README", self.tree / "to_file")
        self.assertEqual(
            manifest(self.tree)["packages"],
            [
                {
                    "name": "@linked/README",
                    "link": "store",
                    "path": "store/README",
                    "file": True,
                },
                {
                    "name": "@linked/readme_link",
                    "link": "README",
                    "path": "store/README",
                    "file": True,
                },
                {
                    "name": "@linked/x",
                    "link": "store",
                    "path": "store/x",
                    "version": "2.0.0",
                },
                {
                    "name": "@scope/y",
                    "link": "../real",
                    "path": "real",
                    "version": "1.0.0",
                },
                {"name": "alias", "link": "real", "path": "real", "version": "1.0.0"},
                {"name": "real", "version": "1.0.0"},
                {"name": "store"},
                {
                    "name": "to_file",
                    "link": "store/README",
                    "path": "store/README",
                    "file": True,
                },
            ],
        )

    @unittest.skipIf(sys.platform == "win32", "creating symlinks needs privileges")
    def test_links_outside_the_tree_or_dangling_record_no_path(self) -> None:
        _write(self.root / "outside" / "package.json", json.dumps({"version": "1.0.0"}))
        self.tree.mkdir()
        os.symlink("../outside", self.tree / "out", target_is_directory=True)
        os.symlink("../outside/package.json", self.tree / "out_file")
        os.symlink("missing", self.tree / "dangling", target_is_directory=True)
        os.symlink(".", self.tree / "self", target_is_directory=True)
        self.assertEqual(
            manifest(self.tree)["packages"],
            [
                {"name": "dangling", "link": "missing"},
                {"name": "out", "link": "../outside", "outside": True},
                {
                    "name": "out_file",
                    "link": "../outside/package.json",
                    "outside": True,
                },
                {"name": "self", "link": ".", "outside": True},
            ],
        )


class TestStripTypes(unittest.TestCase):
    def setUp(self) -> None:
        tmp = tempfile.TemporaryDirectory()
        self.addCleanup(tmp.cleanup)
        self.root = Path(tmp.name)
        self.src = self.root / "src"
        self.out = self.root / "out"

    def test_declarations_and_their_maps_removed(self) -> None:
        for name in (
            "index.js",
            "index.js.map",
            "index.d.ts",
            "index.d.ts.map",
            "esm.mjs",
            "esm.mjs.map",
            "esm.d.mts",
            "esm.d.mts.map",
            "cjs.cjs",
            "cjs.cjs.map",
            "cjs.d.cts",
            "cjs.d.cts.map",
            "main.ts",
        ):
            _write(self.src / "lib" / name, name)
        _write(self.src / "types" / "index.d.ts")
        _write(self.src / "package.json", "{}")
        strip_types(self.src, self.out)
        self.assertEqual(
            _files(self.out),
            [
                "lib/cjs.cjs",
                "lib/cjs.cjs.map",
                "lib/esm.mjs",
                "lib/esm.mjs.map",
                "lib/index.js",
                "lib/index.js.map",
                "lib/main.ts",
                "package.json",
            ],
        )
        self.assertEqual(
            (self.out / "lib" / "index.js.map").read_text(), "index.js.map"
        )
        self.assertEqual(os.listdir(self.out / "types"), [])

    @unittest.skipIf(sys.platform == "win32", "Windows has no executable bit")
    def test_executable_bit_kept(self) -> None:
        _write(self.src / "bin" / "cli.js", "#!/usr/bin/env node")
        os.chmod(self.src / "bin" / "cli.js", 0o755)
        strip_types(self.src, self.out)
        mode = stat.S_IMODE(os.stat(self.out / "bin" / "cli.js").st_mode)
        self.assertEqual(mode & 0o111, 0o111)

    @unittest.skipIf(sys.platform == "win32", "creating symlinks needs privileges")
    def test_symlinks_copied_as_targets(self) -> None:
        _write(self.root / "store" / "pkg" / "index.js", "pkg")
        _write(self.root / "store" / "pkg" / "index.d.ts")
        _write(self.root / "store" / "util.js", "util")
        _write(self.root / "store" / "util.d.ts")
        self.src.mkdir()
        os.symlink("../store/pkg", self.src / "pkg", target_is_directory=True)
        os.symlink("../store/util.js", self.src / "util.js")
        os.symlink("../store/util.d.ts", self.src / "util.d.ts")
        strip_types(self.src, self.out)
        self.assertEqual(_files(self.out), ["pkg/index.js", "util.js"])
        for path in (
            self.out / "pkg",
            self.out / "pkg" / "index.js",
            self.out / "util.js",
        ):
            self.assertFalse(path.is_symlink())
        self.assertEqual((self.out / "pkg" / "index.js").read_text(), "pkg")
        self.assertEqual((self.out / "util.js").read_text(), "util")

    @unittest.skipIf(sys.platform == "win32", "creating symlinks needs privileges")
    def test_symlink_to_declaration_under_other_name_removed(self) -> None:
        _write(self.root / "store" / "index.d.ts")
        _write(self.root / "store" / "index.d.ts.map")
        self.src.mkdir()
        os.symlink("../store/index.d.ts", self.src / "index.js")
        os.symlink("../store/index.d.ts.map", self.src / "index.js.map")
        strip_types(self.src, self.out)
        self.assertEqual(_files(self.out), [])

    @unittest.skipIf(sys.platform == "win32", "creating symlinks needs privileges")
    def test_directory_linked_twice_copied_twice(self) -> None:
        _write(self.root / "store" / "index.js")
        self.src.mkdir()
        os.symlink("../store", self.src / "a", target_is_directory=True)
        os.symlink("../store", self.src / "b", target_is_directory=True)
        strip_types(self.src, self.out)
        self.assertEqual(_files(self.out), ["a/index.js", "b/index.js"])

    @unittest.skipIf(sys.platform == "win32", "creating symlinks needs privileges")
    def test_symlink_cycle_fails(self) -> None:
        for target in (".", ".."):
            with self.subTest(target=target):
                tmp = tempfile.TemporaryDirectory()
                self.addCleanup(tmp.cleanup)
                root = Path(tmp.name)
                _write(root / "src" / "pkg" / "index.js")
                os.symlink(
                    target, root / "src" / "pkg" / "loop", target_is_directory=True
                )
                with self.assertRaisesRegex(ValueError, "symlink cycle: .*loop"):
                    strip_types(root / "src", root / "out")

    @unittest.skipIf(sys.platform == "win32", "creating symlinks needs privileges")
    def test_dangling_symlink_fails(self) -> None:
        self.src.mkdir()
        os.symlink("missing.js", self.src / "index.js")
        with self.assertRaises(FileNotFoundError):
            strip_types(self.src, self.out)

    @unittest.skipIf(sys.platform == "win32", "creating symlinks needs privileges")
    def test_cli_reports_dangling_symlink(self) -> None:
        self.src.mkdir()
        os.symlink("missing.js", self.src / "index.js")
        argv = ["package_tree.py", "strip-types", "--src", str(self.src)]
        stderr = io.StringIO()
        with (
            mock.patch.object(sys, "argv", argv + ["--out", str(self.out)]),
            contextlib.redirect_stderr(stderr),
        ):
            self.assertEqual(main(), 1)
        self.assertRegex(stderr.getvalue(), r"^strip-types: .*index\.js'\n$")


class TestValidatePackage(unittest.TestCase):
    def setUp(self) -> None:
        tmp = tempfile.TemporaryDirectory()
        self.addCleanup(tmp.cleanup)
        self.package = Path(tmp.name) / "pkg"
        self.package.mkdir()

    def _manifest(self, **fields: object) -> None:
        (self.package / "package.json").write_text(json.dumps(fields))

    def _validate(
        self,
        name: str = "pkg",
        dir_name: str | None = None,
        main: str | None = None,
        bins: dict[str, str] | None = None,
        type_: str | None = None,
        version: str | None = None,
        module: Path | None = None,
        module_src_root: str = "",
    ) -> list[str]:
        return validate_package(
            self.package,
            name,
            dir_name,
            main,
            bins or {},
            type_,
            version,
            module,
            module_src_root,
        )

    def test_valid_package(self) -> None:
        _write(self.package / "dist" / "index.js")
        _write(self.package / "dist" / "index.d.ts")
        _write(self.package / "dist" / "cli.js", "#!/usr/bin/env node\n")
        self._manifest(
            name="pkg",
            version="1.0.0",
            type="module",
            main="./dist/index.js",
            types="./dist/index.d.ts",
            bin={"pkg-cli": "dist/cli.js"},
            exports={
                ".": {"types": "./dist/index.d.ts", "default": "./dist/index.js"},
                "./cli": ["./dist/cli.js"],
                "./*": {"types": "./dist/*.d.ts", "default": "./dist/*.js"},
                "./internal/*": None,
                "./package.json": "./package.json",
            },
        )
        self.assertEqual(
            self._validate(
                main="dist/index.js",
                bins={"pkg-cli": "dist/cli.js"},
                type_="module",
                version="1.0.0",
            ),
            [],
        )

    def test_missing_or_malformed_package_json(self) -> None:
        self.assertEqual(
            self._validate(),
            [
                "the package has no package.json: a `prebuilt` directory must contain one, or `package_json` adds it"
            ],
        )
        (self.package / "package.json").write_text("{")
        [message] = self._validate()
        self.assertTrue(message.startswith("package.json is not valid JSON: "), message)
        (self.package / "package.json").write_text("[]")
        self.assertEqual(self._validate(), ["package.json must contain a JSON object"])

    def test_name_must_be_package_name_or_out(self) -> None:
        for manifest_name, dir_name, errors in [
            ("pkg", None, []),
            ("pkg", "canon/pkg", []),
            ("canon/pkg", "canon/pkg", []),
            (
                "other",
                None,
                ["package.json \"name\" 'other' must be the `package_name` 'pkg'"],
            ),
            (
                "other",
                "canon/pkg",
                [
                    "package.json \"name\" 'other' must be the `package_name` 'pkg' or the `out` 'canon/pkg'"
                ],
            ),
            (
                None,
                None,
                ["package.json \"name\" null must be the `package_name` 'pkg'"],
            ),
        ]:
            with self.subTest(name=manifest_name, dir_name=dir_name):
                self._manifest(name=manifest_name)
                self.assertEqual(self._validate(dir_name=dir_name), errors)
        self._manifest()
        self.assertEqual(
            self._validate(),
            ["package.json has no \"name\"; it must be the `package_name` 'pkg'"],
        )

    def test_main_resolves_as_node_resolves_it(self) -> None:
        _write(self.package / "index.js")
        _write(self.package / "lib" / "index.js")
        _write(self.package / "lib" / "util.cjs")
        _write(self.package / "data.json")
        for main_field in [".", "./", "index", "lib", "./lib/", "lib/util.cjs", "data"]:
            with self.subTest(main=main_field):
                self._manifest(name="pkg", main=main_field)
                self.assertEqual(self._validate(), [])
        for main_field in [
            "missing.js",
            "../pkg/index.js",
            "/index.js",
            "lib/util",
            "Index.js",
        ]:
            with self.subTest(main=main_field):
                self._manifest(name="pkg", main=main_field)
                self.assertEqual(
                    self._validate(),
                    [
                        f"package.json \"main\" '{main_field}' is not a file in the package"
                    ],
                )
        self._manifest(name="pkg", main=["index.js"])
        self.assertEqual(
            self._validate(),
            ['package.json "main" must be a string, not ["index.js"]'],
        )

    def test_bin_files_exist_and_start_with_a_shebang(self) -> None:
        _write(self.package / "cli.js", "#!/usr/bin/env node\n")
        _write(self.package / "plain.js", "console.log(1);\n")
        _write(self.package / "empty.js")
        _write(self.package / "bom.js", "\ufeff#!/usr/bin/env node\n")
        self._manifest(name="pkg", bin={"bom": "bom.js"})
        self.assertEqual(self._validate(), [])
        self._manifest(name="@scope/pkg", bin="./cli.js")
        self.assertEqual(self._validate(name="@scope/pkg"), [])
        self.assertEqual(self._validate(name="@scope/pkg", bins={"pkg": "cli.js"}), [])
        self.assertEqual(
            self._validate(name="@scope/pkg", bins={"tool": "cli.js"}),
            ['package.json "bin" \'./cli.js\' does not match `bin` {"tool": "cli.js"}'],
        )
        self._manifest(bin="./cli.js")
        self.assertEqual(
            self._validate(bins={"pkg": "cli.js"}),
            [
                "package.json has no \"name\"; it must be the `package_name` 'pkg'",
                'package.json "bin" is a string, which names its command after "name", so "name" must be a string',
            ],
        )
        self._manifest(name="@scope/pkg", bin="./plain.js")
        self.assertEqual(
            self._validate(name="@scope/pkg"),
            [
                "package.json \"bin\" 'pkg' './plain.js' does not start with a shebang ('#!')"
            ],
        )
        self._manifest(
            name="pkg",
            bin={
                "ok": "cli.js",
                "plain": "plain.js",
                "empty": "./empty.js",
                "missing": "missing.js",
                "dir": ".",
                "number": 1,
            },
        )
        self.assertEqual(
            self._validate(),
            [
                "package.json \"bin\" 'number' must be a string, not 1",
                "package.json \"bin\" 'plain' 'plain.js' does not start with a shebang ('#!')",
                "package.json \"bin\" 'empty' './empty.js' does not start with a shebang ('#!')",
                "package.json \"bin\" 'missing' 'missing.js' is not a file in the package",
                "package.json \"bin\" 'dir' '.' is not a file in the package",
            ],
        )
        self._manifest(name="pkg", bin=["cli.js"])
        self.assertEqual(
            self._validate(), ['package.json "bin" must be a string or an object']
        )
        self.assertEqual(
            self._validate(bins={"tool": "cli.js"}),
            ['package.json "bin" must be a string or an object'],
        )
        self._manifest(name="pkg", bin={"tool": "cli.js", "number": 1})
        self.assertEqual(
            self._validate(bins={"tool": "cli.js"}),
            ["package.json \"bin\" 'number' must be a string, not 1"],
        )
        self.assertEqual(
            self._validate(bins={"tool": "cli.js", "number": "cli.js"}),
            ["package.json \"bin\" 'number' must be a string, not 1"],
        )
        self.assertEqual(
            self._validate(bins={"tool": "plain.js"}),
            [
                "package.json \"bin\" 'number' must be a string, not 1",
                'package.json "bin" {"tool": "cli.js", "number": 1} does not match `bin` {"tool": "plain.js"}',
            ],
        )
        self.assertEqual(
            self._validate(bins={"tool": "cli.js", "other": "cli.js"}),
            [
                "package.json \"bin\" 'number' must be a string, not 1",
                'package.json "bin" {"tool": "cli.js", "number": 1} does not match `bin` {"tool": "cli.js", "other": "cli.js"}',
            ],
        )

    def test_literal_exports_targets_exist(self) -> None:
        _write(self.package / "dist" / "index.js")
        self._manifest(
            name="pkg",
            exports={
                ".": {
                    "types": "./dist/index.d.ts",
                    "node": {
                        "import": "./dist/index.mjs",
                        "default": "./dist/index.js",
                    },
                },
                "./a": ["./dist/a.js", "./dist/index.js"],
                "./b": "dist/index.js",
                "./c": "./dist/../../outside.js",
                "./d": 1,
                "./patterns/*": "./missing/*.js",
            },
        )
        self.assertEqual(
            self._validate(),
            [
                'package.json exports["."]["types"] \'./dist/index.d.ts\' is not a file in the package',
                'package.json exports["."]["node"]["import"] \'./dist/index.mjs\' is not a file in the package',
                "package.json exports[\"./a\"][0] './dist/a.js' is not a file in the package",
                "package.json exports[\"./b\"] 'dist/index.js' must start with './'",
                "package.json exports[\"./c\"] './dist/../../outside.js' is not a file in the package",
                'package.json exports["./d"] must be a string, an array, an object or null',
            ],
        )
        self._manifest(name="pkg", exports="./dist/missing.js")
        self.assertEqual(
            self._validate(),
            ["package.json exports './dist/missing.js' is not a file in the package"],
        )

    def test_exports_conditions_list_types_first(self) -> None:
        _write(self.package / "index.js")
        _write(self.package / "index.d.ts")
        self._manifest(
            name="pkg",
            exports={
                "import": {"default": "./index.js", "types": "./index.d.ts"},
                "types": "./index.d.ts",
            },
        )
        self.assertEqual(
            self._validate(),
            [
                'package.json exports lists "types" after "import"; "types" must come first',
                'package.json exports["import"] lists "types" after "default"; "types" must come first',
            ],
        )

    def test_exports_cannot_mix_subpaths_and_conditions(self) -> None:
        _write(self.package / "index.js")
        self._manifest(name="pkg", exports={".": "./index.js", "default": "./index.js"})
        self.assertEqual(
            self._validate(),
            [
                "package.json \"exports\" mixes subpaths (keys starting with '.') and conditions"
            ],
        )

    def test_declared_attributes_match_package_json(self) -> None:
        _write(self.package / "dist" / "index.js")
        _write(self.package / "dist" / "cli.js", "#!/usr/bin/env node\n")
        self._manifest(
            name="pkg",
            version="1.0.0",
            main="./dist/index.js",
            bin={"cli": "./dist/cli.js"},
        )
        self.assertEqual(
            self._validate(
                main="dist/index.js",
                bins={"cli": "dist/cli.js"},
                type_="commonjs",
                version="1.0.0",
            ),
            [],
        )
        self.assertEqual(
            self._validate(
                main="dist/cli.js",
                bins={"tool": "dist/cli.js"},
                type_="module",
                version="2.0.0",
            ),
            [
                "package.json \"main\" './dist/index.js' does not match `main` 'dist/cli.js'",
                'package.json "bin" {"cli": "./dist/cli.js"} does not match `bin` {"tool": "dist/cli.js"}',
                "package.json has no \"type\", but `type` is 'module'",
                "package.json \"version\" '1.0.0' does not match `version` '2.0.0'",
            ],
        )
        self.assertEqual(self._validate(bins={"cli": "./dist/../dist/cli.js"}), [])
        self._manifest(name="pkg", main="./dist/index")
        self.assertEqual(self._validate(main="dist/index.js"), [])
        self.assertEqual(self._validate(main="dist"), [])
        self._manifest(name="pkg", type="module")
        self.assertEqual(
            self._validate(main="dist/index.js", bins={"cli": "dist/cli.js"}),
            [
                "package.json has no \"main\", but `main` is 'dist/index.js'",
                'package.json has no "bin", but `bin` is {"cli": "dist/cli.js"}',
            ],
        )

    def test_types_are_files_in_the_package(self) -> None:
        _write(self.package / "index.d.ts")
        _write(self.package / "dist" / "index.d.ts")
        _write(self.package / "dist" / "util.d.ts")
        for field in ["types", "typings"]:
            for value in [".", "./dist/index.d.ts", "dist/util", "dist"]:
                with self.subTest(field=field, value=value):
                    self._manifest(name="pkg", **{field: value})
                    self.assertEqual(self._validate(), [])
            for value in ["dist/missing.d.ts", "dist/Util.d.ts", "../pkg/index.d.ts"]:
                with self.subTest(field=field, value=value):
                    self._manifest(name="pkg", **{field: value})
                    self.assertEqual(
                        self._validate(),
                        [
                            f"package.json \"{field}\" '{value}' is not a file in the package"
                        ],
                    )
            self._manifest(name="pkg", **{field: ["index.d.ts"]})
            self.assertEqual(
                self._validate(),
                [f'package.json "{field}" must be a string, not ["index.d.ts"]'],
            )

    def test_type_matches_the_module_package_json(self) -> None:
        module = self.package.parent / "module"
        (module / "src").mkdir(parents=True)
        self._manifest(name="pkg", type="commonjs")
        self.assertEqual(self._validate(module=module), [])
        (module / "package.json").write_text(json.dumps({"type": "module"}))
        self.assertEqual(
            self._validate(module=module, module_src_root="src"),
            [
                "package.json \"type\" 'commonjs' does not match the \"type\" 'module' of the module's package.json, "
                "which sets the module format `tsc` emits for sources without a closer package.json; "
                'set `type`, or the "type" of `package_json`, to match'
            ],
        )
        self._manifest(name="pkg", type="module")
        self.assertEqual(self._validate(module=module), [])
        (module / "src" / "package.json").write_text("{}")
        self.assertEqual(
            self._validate(module=module, module_src_root="src"),
            [
                "package.json \"type\" 'module' does not match the \"type\" 'commonjs' of the module's src/package.json, "
                "which sets the module format `tsc` emits for sources without a closer package.json; "
                'set `type`, or the "type" of `package_json`, to match'
            ],
        )
        self._manifest(name="pkg")
        self.assertEqual(self._validate(module=module, module_src_root="src"), [])

    def test_unusable_module_package_json(self) -> None:
        module = self.package.parent / "module"
        module.mkdir()
        self._manifest(name="pkg")
        (module / "package.json").write_text("{")
        [message] = self._validate(module=module)
        self.assertTrue(
            message.startswith("the module's package.json is not valid JSON: "), message
        )
        (module / "package.json").write_text("[]")
        self.assertEqual(
            self._validate(module=module),
            ["the module's package.json must contain a JSON object"],
        )

    def test_cli_values_may_start_with_a_dash(self) -> None:
        _write(self.package / "-entry.js")
        _write(self.package / "bin" / "-x.js", "#!/usr/bin/env node\n")
        self._manifest(
            name="-pkg", main="-entry.js", bin={"-x": "bin/-x.js"}, version="-1"
        )
        out = self.package.parent / "result.json"
        argv = [
            "package_tree.py",
            "validate-package",
            f"--package={self.package}",
            "--name=-pkg",
            "--main=-entry.js",
            "--bin=-x:bin/-x.js",
            "--version=-1",
            f"--out={out}",
        ]
        with mock.patch.object(sys, "argv", argv):
            self.assertEqual(main(), 0)
        self.assertEqual(
            json.loads(out.read_text()), {"version": 1, "data": {"status": "success"}}
        )

    def test_write_validation_result(self) -> None:
        out = self.package / "result.json"
        write_validation_result(out, [])
        self.assertEqual(
            json.loads(out.read_text()), {"version": 1, "data": {"status": "success"}}
        )
        write_validation_result(out, ["a", "b"])
        self.assertEqual(
            json.loads(out.read_text()),
            {"version": 1, "data": {"status": "failure", "message": "a\nb"}},
        )
