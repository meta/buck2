# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

# pyre-strict

import os
import shutil
from datetime import datetime, timedelta
from pathlib import Path

from buck2.tests.e2e_util.api.buck import Buck
from buck2.tests.e2e_util.buck_workspace import buck_test, BuckTestCase


class UnhashedOutputsTest(BuckTestCase):
    @buck_test()
    async def test_unhashed_putputs(self, buck: Buck) -> None:
        await buck.build("//pack:trivial_build")

        p = buck.cwd / "buck-out" / "v2" / "gen" / "root" / "pack" / "foo.txt"
        assert p.exists()
        assert p.is_symlink()

    @buck_test()
    async def test_projected_output(self, buck: Buck) -> None:
        await buck.build("//:projected_output")

        p = buck.cwd / "buck-out" / "v2" / "gen" / "root" / "dir"
        assert p.exists()
        assert p.is_symlink()
        assert (p / "file").is_file()

    @buck_test()
    async def test_materializer_managed_unhashed_output(self, buck: Buck) -> None:
        await buck.build("//pack:trivial_build")

        p = buck.cwd / "buck-out" / "v2" / "gen" / "root" / "pack" / "foo.txt"
        assert p.is_symlink()

        materializer_state = await buck.audit("deferred-materializer", "list")
        assert "buck-out/v2/gen/root/pack/foo.txt" in materializer_state.stdout

        future_time = int((datetime.now() + timedelta(weeks=7)).timestamp())
        await buck.clean(f"--keep-since-time={future_time}")
        assert p.is_symlink()
        assert p.is_file()
        materializer_state = await buck.audit("deferred-materializer", "list")
        assert "buck-out/v2/gen/root/pack/foo.txt" in materializer_state.stdout

    @buck_test(skip_for_materializer=["v2"])
    async def test_materializer_managed_unhashed_output_without_materialization(
        self,
        buck: Buck,
    ) -> None:
        await buck.build("//pack:trivial_build", "--materializations=none")
        unhashed = buck.cwd / "buck-out" / "v2" / "gen" / "root" / "pack" / "foo.txt"

        assert not unhashed.is_symlink()

        materializer_state = await buck.audit("deferred-materializer", "list")
        assert "buck-out/v2/gen/root/pack/foo.txt" in materializer_state.stdout

    @buck_test()
    async def test_build_symlink_does_not_traverse_existing_symlinks(
        self, buck: Buck
    ) -> None:
        await buck.build("//pack:trivial_build")
        symlink_folder = buck.cwd / "buck-out" / "v2" / "gen" / "root" / "pack"

        # Now, overwrite part of the symlink path with something we cannot traverse.
        path = symlink_folder.parent
        shutil.rmtree(path)
        # On Windows this is just non existing path.
        os.symlink("/dev/null", path)

        # Can we still build? If we delete the symlink when walking up the path, we
        # can. If we traverse it, we can't.
        await buck.build("//pack:trivial_build")

    @buck_test()
    async def test_conflict_with_content_based_paths(self, buck: Buck) -> None:
        symlink_path: Path = (
            buck.cwd / "buck-out" / "v2" / "gen" / "root" / "conflict" / "shared_name"
        )
        content_based_path: Path = (
            buck.cwd / "buck-out" / "v2" / "art" / "root" / "conflict" / "shared_name"
        )
        subtarget_output: Path
        # sanity check that we're starting from a clean state
        assert not symlink_path.exists()
        assert not content_based_path.exists()

        def base_checks(*, should_symlink_exist: bool) -> None:
            if should_symlink_exist:
                assert symlink_path.is_symlink()
                assert symlink_path.resolve().is_file()
            else:
                assert not symlink_path.exists()

            assert content_based_path.is_dir()
            assert subtarget_output.is_symlink()
            assert subtarget_output.resolve().is_file()
            assert not subtarget_output.resolve().is_relative_to(symlink_path)
            assert subtarget_output.resolve().is_relative_to(content_based_path)
            # Verify we can read the contents of the file
            with open(subtarget_output) as f:
                f.read()

        #
        # Build just the subtarget. Esnsure that the subtarget output exists and is
        # reacable, and that it lives in the place we expect.
        #
        res = await buck.build(
            "//conflict/shared_name:subtarget",
            "--config",
            "buck2.create_unhashed_links=false",
        )
        subtarget_output = res.get_build_report().output_for_target(
            "root//conflict/shared_name:subtarget"
        )
        base_checks(should_symlink_exist=False)

        #
        # Build the conflicting target w/o unhashed links. This should leave the
        # subtarget_output alone, which should remain readable.
        #
        await buck.build(
            "//conflict:shared_name",
            "--config",
            "buck2.create_unhashed_links=false",
        )
        # TODO(jtbraun): this should instead ensure the symlink does NOT exist, and the content_based_path does and is a folder
        base_checks(should_symlink_exist=False)

        #
        # Build the conflicting target with unhashed links. This will overwrite the
        # subtarget with a directory, and the symlink_path will now exist.
        #
        await buck.build("//conflict:shared_name")
        base_checks(should_symlink_exist=True)

    @buck_test()
    async def test_projected_symlink_output(self, buck: Buck) -> None:
        result = await buck.build("//:projected_symlink_output")
        output = result.get_build_report().output_for_target(
            "root//:projected_symlink_output"
        )

        assert output.parent.is_symlink()
        assert output.is_symlink()
        assert output.resolve().is_file()

    @buck_test(
        skip_for_os=["windows"],
        extra_buck_config={"buck2": {"materializer_strict_leases": "false"}},
    )
    async def test_content_based_output_keeps_mtimes(self, buck: Buck) -> None:
        # The published tree contains fixed file, symlink, empty-directory, and root mtimes.
        # Forced rematerialization discards timestamps, so the published tree must stay on disk.
        # A rerun with identical contents must keep the published tree while readers can use it.
        first_inodes = {}
        for base in (1_000_000_000, 1_000_000_100):
            result = await buck.build(
                "//:projected_symlink_output", "--no-buckd", "-c", f"test.mtime={base}"
            )
            out = (
                result.get_build_report()
                .output_for_target("root//:projected_symlink_output")
                .parent
            )
            for i, name in enumerate(("target", "profile", "sub", ".")):
                # os.path.join retains the trailing `.`, which Path would remove from a symlink.
                metadata = os.lstat(os.path.join(out, name))
                assert metadata.st_mtime == 1_000_000_000 + i, (base, name, metadata.st_mtime)
                if base == 1_000_000_000:
                    first_inodes[name] = metadata.st_ino
                else:
                    assert metadata.st_ino == first_inodes[name], name
