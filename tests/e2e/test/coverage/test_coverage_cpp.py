# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

import json
from pathlib import Path

from buck2.tests.e2e_util.api.buck import Buck
from buck2.tests.e2e_util.buck_workspace import buck_test, BuckTestCase

from .test_coverage_utils import collect_coverage_for


class CoverageCppTest(BuckTestCase):
    async def _compile_commands(
        self, buck: Buck, target: str, selected_files: list[str] | None
    ) -> dict[str, str]:
        args = []
        if selected_files is not None:
            args.extend(
                [
                    "-c",
                    "code_coverage.enable=filtered",
                    "-c",
                    "code_coverage.file_path_filter=" + ":".join(selected_files),
                ]
            )
        result = await buck.aquery(
            f"kind(run, deps('{target}'))",
            "--output-attribute=^(category|identifier|cmd)$",
            "--json",
            *args,
        )
        commands = {}
        for key, action in json.loads(result.stdout).items():
            if action["category"] == "cxx_compile":
                label = key.split("`")[1].split(" (")[0]
                commands[label + " " + action["identifier"]] = action["cmd"]
        assert commands
        return commands

    @buck_test(inplace=True)
    async def test_cpp_coverage_private_dep_cache_reuse(self, buck: Buck) -> None:
        package = "fbcode//testing_frameworks/code_coverage/playground"
        source_dir = "fbcode/testing_frameworks/code_coverage/playground/"
        header = source_dir + "ThirdLevelDep.h"
        ordinary = await self._compile_commands(buck, package + ":test", None)
        selected = await self._compile_commands(buck, package + ":test", [header])
        unchanged = (package + ":first_level_dep ", package + ":test ")
        instrumented = (
            package + ":third_level_dep ",
            package + ":second_level_dep_that_includes_third_level_dep ",
        )
        for prefix in unchanged + instrumented:
            assert any(key.startswith(prefix) for key in selected), prefix
        for key, command in selected.items():
            if key.startswith(unchanged):
                assert command == ordinary[key]
            elif key.startswith(instrumented):
                assert "-fprofile-instr-generate" in command

    @buck_test(inplace=True)
    async def test_cpp_test_coverage(self, buck: Buck, tmp_path: Path) -> None:
        coverage_file = tmp_path / "coverage.txt"
        await buck.test(
            "@fbcode//mode/dbgo-cov",
            "fbcode//buck2/tests/targets/rules/cxx:cpp_test_pass",
            "--",
            "--collect-coverage",
            f"--coverage-output={coverage_file}",
        )
        paths = []
        with open(coverage_file) as results:
            for line in results:
                paths.append(json.loads(line)["filepath"])

        assert "fbcode/buck2/tests/targets/rules/cxx/cpp_test_pass.cpp" in paths, str(
            paths
        )
        assert "fbcode/common/gtest/LightMain.cpp" in paths, str(paths)

    @buck_test(inplace=True)
    async def test_cpp_test_coverage_filter_by_path_outside_target(
        self,
        buck: Buck,
        tmp_path: Path,
    ) -> None:
        paths = await collect_coverage_for(
            buck,
            tmp_path,
            "fbcode//buck2/tests/targets/rules/cxx:cpp_test_pass",
            folder_filter=["xplat/folly"],
            file_filter=[],
        )

        expected_paths = [p for p in paths if p.startswith("xplat/folly")]
        assert len(expected_paths) > 0, str(paths)
        # third-party headers transitively included by folly now appear with real
        # source paths due to -fcoverage-prefix-map; allow them.
        unexpected_paths = [
            p
            for p in paths
            if not p.startswith("xplat/folly")
            and not p.startswith("fbcode/fbsource/folly/")
            and not p.startswith("third-party/fast_float/")
            and not p.startswith("third-party/libdwarf/")
        ]
        assert len(unexpected_paths) == 0, str(paths)

    @buck_test(inplace=True)
    async def test_cpp_test_coverage_filter_by_path_of_target(
        self, buck: Buck, tmp_path: Path
    ) -> None:
        paths = await collect_coverage_for(
            buck,
            tmp_path,
            "fbcode//buck2/tests/targets/rules/cxx:cpp_test_pass",
            folder_filter=["fbcode/buck2/tests"],
            file_filter=[],
        )

        expected_paths = [p for p in paths if p.startswith("fbcode/buck2/tests")]
        assert len(expected_paths) > 0, str(paths)
        unexpected_paths = [p for p in paths if not p.startswith("fbcode/buck2/tests")]
        assert len(unexpected_paths) == 0, str(paths)

    @buck_test(inplace=True)
    async def test_cpp_test_coverage_filter_by_path_of_target_with_dev_lg(
        self,
        buck: Buck,
        tmp_path: Path,
    ) -> None:
        paths = await collect_coverage_for(
            buck,
            tmp_path,
            "fbcode//buck2/tests/targets/rules/cxx:cpp_test_pass",
            mode="@fbcode//mode/dev-lg",
            folder_filter=["fbcode/buck2/tests"],
            file_filter=[],
        )

        expected_paths = [p for p in paths if p.startswith("fbcode/buck2/tests")]
        assert len(expected_paths) > 0, str(paths)
        unexpected_paths = [p for p in paths if not p.startswith("fbcode/buck2/tests")]
        assert len(unexpected_paths) == 0, str(paths)

    @buck_test(inplace=True)
    async def test_cpp_test_coverage_filter_by_path_in_link_group_with_dev_lg(
        self,
        buck: Buck,
        tmp_path: Path,
    ) -> None:
        paths = await collect_coverage_for(
            buck,
            tmp_path,
            "fbcode//buck2/tests/targets/rules/cxx:cpp_test_pass",
            mode="@fbcode//mode/dev-lg",
            folder_filter=["xplat/folly"],
            file_filter=[],
        )

        expected_paths = [p for p in paths if p.startswith("xplat/folly")]
        assert len(expected_paths) > 0, str(paths)
        unexpected_paths = [
            p
            for p in paths
            if not p.startswith("xplat/folly")
            and not p.startswith("fbcode/fbsource/folly/")
            and not p.startswith("third-party/fast_float/")
            and not p.startswith("third-party/libdwarf/")
        ]
        assert len(unexpected_paths) == 0, str(paths)

    @buck_test(inplace=True)
    async def test_cpp_test_coverage_filter_by_file_of_target_with_dev_lg(
        self,
        buck: Buck,
        tmp_path: Path,
    ) -> None:
        source_name = "fbcode/buck2/tests/targets/rules/cxx/cpp_test_pass.cpp"
        paths = await collect_coverage_for(
            buck,
            tmp_path,
            "fbcode//buck2/tests/targets/rules/cxx:cpp_test_pass",
            mode="@fbcode//mode/dev-lg",
            folder_filter=[],
            file_filter=[source_name],
        )

        expected_paths = [p for p in paths if p == source_name]
        assert len(expected_paths) > 0, str(paths)
        unexpected_paths = [p for p in paths if p != source_name]
        assert len(unexpected_paths) == 0, str(paths)

    @buck_test(inplace=True)
    async def test_cpp_test_coverage_filter_by_source_file_in_link_group_with_dev_lg(
        self,
        buck: Buck,
        tmp_path: Path,
    ) -> None:
        source_name = "xplat/folly/String.cpp"
        paths = await collect_coverage_for(
            buck,
            tmp_path,
            "fbcode//buck2/tests/targets/rules/cxx:cpp_test_pass",
            mode="@fbcode//mode/dev-lg",
            folder_filter=[],
            file_filter=[source_name],
        )

        expected_paths = [p for p in paths if p == source_name]
        assert len(expected_paths) > 0, str(paths)
        unexpected_paths = [
            p for p in paths if not p == source_name and not p.endswith(".h")
        ]
        assert len(unexpected_paths) == 0, str(paths)

    @buck_test(inplace=True)
    async def test_cpp_test_coverage_filter_by_header_file_in_link_group_with_dev_lg(
        self,
        buck: Buck,
        tmp_path: Path,
    ) -> None:
        header_name = "fbcode/testing_frameworks/code_coverage/playground/link_groups/LibraryRightRightOnlyUsedHere.h"
        paths = await collect_coverage_for(
            buck,
            tmp_path,
            "fbcode//testing_frameworks/code_coverage/playground/link_groups:test_with_link_groups",
            mode="@fbcode//mode/dev-lg",
            folder_filter=[],
            file_filter=[header_name],
        )

        assert paths == []

    @buck_test(inplace=True)
    async def test_cpp_test_coverage_filter_by_header_file_defined_in_one_link_group_and_used_in_another(
        self,
        buck: Buck,
        tmp_path: Path,
    ) -> None:
        header_name = "fbcode/testing_frameworks/code_coverage/playground/link_groups/LibraryRightLeftUsedInOtherLinkGroup.h"
        paths = await collect_coverage_for(
            buck,
            tmp_path,
            "fbcode//testing_frameworks/code_coverage/playground/link_groups:test_with_header_used_in_different_link_group",
            mode="@fbcode//mode/dev-lg",
            folder_filter=[],
            file_filter=[header_name],
        )

        assert paths == [header_name]

    @buck_test(inplace=True)
    async def test_cpp_test_coverage_filter_by_header_file_defined_in_one_link_group_and_used_in_test_binary_link_group(
        self,
        buck: Buck,
        tmp_path: Path,
    ) -> None:
        header_name = "fbcode/testing_frameworks/code_coverage/playground/link_groups/LibraryRightLeftUsedInOtherLinkGroup.h"
        paths = await collect_coverage_for(
            buck,
            tmp_path,
            "fbcode//testing_frameworks/code_coverage/playground/link_groups:test_with_header_used_in_test_binary",
            mode="@fbcode//mode/dev-lg",
            folder_filter=[],
            file_filter=[header_name],
        )

        assert paths == [header_name]

    @buck_test(inplace=True)
    async def test_cpp_test_coverage_filter_by_file(
        self, buck: Buck, tmp_path: Path
    ) -> None:
        source_name = "fbcode/buck2/tests/targets/rules/cxx/cpp_test_pass.cpp"
        paths = await collect_coverage_for(
            buck,
            tmp_path,
            "fbcode//buck2/tests/targets/rules/cxx:cpp_test_pass",
            folder_filter=[],
            file_filter=[source_name],
        )

        assert len(paths) == 1, str(paths)
        assert paths[0] == source_name

    @buck_test(inplace=True)
    async def test_cpp_test_coverage_when_filter_by_test_binary_header_file(
        self, buck: Buck, tmp_path: Path
    ) -> None:
        header_name = "fbcode/testing_frameworks/code_coverage/playground/Test.h"
        paths = await collect_coverage_for(
            buck,
            tmp_path,
            "fbcode//testing_frameworks/code_coverage/playground:test",
            folder_filter=[],
            file_filter=[header_name],
        )

        assert paths == [header_name]

    @buck_test(inplace=True)
    async def test_cpp_test_coverage_when_filter_by_library_header_file(
        self, buck: Buck, tmp_path: Path
    ) -> None:
        header_name = (
            "fbcode/testing_frameworks/code_coverage/playground/ThirdLevelDep.h"
        )
        paths = await collect_coverage_for(
            buck,
            tmp_path,
            "fbcode//testing_frameworks/code_coverage/playground:test",
            folder_filter=[],
            file_filter=[header_name],
        )

        assert paths == [header_name]

    @buck_test(inplace=True)
    async def test_cpp_test_coverage_when_filter_by_library_private_header_file(
        self, buck: Buck, tmp_path: Path
    ) -> None:
        private_header_name = (
            "fbcode/testing_frameworks/code_coverage/playground/ThirdLevelDepPrivate.h"
        )
        paths = await collect_coverage_for(
            buck,
            tmp_path,
            "fbcode//testing_frameworks/code_coverage/playground:test",
            folder_filter=[],
            file_filter=[private_header_name],
        )

        assert len(paths) == 3, str(paths)
        assert private_header_name in paths
        assert (
            "fbcode/testing_frameworks/code_coverage/playground/ThirdLevelDep.cpp"
            in paths
        )
        assert (
            "fbcode/testing_frameworks/code_coverage/playground/ThirdLevelDepPrivate.h"
            in paths
        )

    @buck_test(inplace=True)
    async def test_cpp_test_coverage_when_filter_by_header_file_in_headers_only_library(
        self, buck: Buck, tmp_path: Path
    ) -> None:
        header_name = "fbcode/testing_frameworks/code_coverage/playground/LibraryWithOnlyHeaders.h"
        paths = await collect_coverage_for(
            buck,
            tmp_path,
            "fbcode//testing_frameworks/code_coverage/playground:test_with_dep_with_only_headers",
            folder_filter=[],
            file_filter=[header_name],
        )

        assert paths == [header_name]

    @buck_test(inplace=True)
    async def test_cpp_test_coverage_filter_by_file_with_opt_mode(
        self,
        buck: Buck,
        tmp_path: Path,
    ) -> None:
        source_name = "fbcode/buck2/tests/targets/rules/cxx/cpp_test_pass.cpp"
        paths = await collect_coverage_for(
            buck,
            tmp_path,
            target="fbcode//buck2/tests/targets/rules/cxx:cpp_test_pass",
            folder_filter=[],
            file_filter=[source_name],
            mode="@fbcode//mode/opt",
        )

        assert len(paths) == 1, str(paths)
        assert paths[0] == source_name

    @buck_test(inplace=True)
    async def test_cpp_test_coverage_filter_by_file_and_path(
        self, buck: Buck, tmp_path: Path
    ) -> None:
        source_name = "fbcode/buck2/tests/targets/rules/cxx/cpp_test_pass.cpp"
        paths = await collect_coverage_for(
            buck,
            tmp_path,
            "fbcode//buck2/tests/targets/rules/cxx:cpp_test_pass",
            folder_filter=["xplat/folly"],
            file_filter=[source_name],
        )

        folly_paths = [p for p in paths if p.startswith("xplat/folly")]
        assert len(folly_paths) > 0, f"expected to find some folly sources in {paths}"

        source_paths = [p for p in paths if p == source_name]
        assert len(source_paths) == 1, f"expected to find {source_name} in {paths}"

        unexpected_paths = [
            p
            for p in paths
            if p != source_name
            and not p.startswith("xplat/folly")
            and not p.startswith("fbcode/fbsource/folly/")
            and not p.startswith("third-party/fast_float/")
            and not p.startswith("third-party/libdwarf/")
        ]
        assert len(unexpected_paths) == 0, (
            f"Only coverage for the test source file and files under xplat/folly should have been collected, but got {unexpected_paths}"
        )

    @buck_test(inplace=True)
    async def test_cpp_test_coverage_xplat_filter_by_file_path(
        self,
        buck: Buck,
        tmp_path: Path,
    ) -> None:
        file_to_collect_coverage = "xplat/testinfra/playground/cpp/ExampleTest.cpp"
        paths = await collect_coverage_for(
            buck,
            tmp_path,
            "fbsource//xplat/testinfra/playground/cpp:example_testFbcode",
            folder_filter=[],
            file_filter=[file_to_collect_coverage],
        )

        assert set(paths) == {
            file_to_collect_coverage,
        }, (
            f"Should collect coverage only for {file_to_collect_coverage} but got coverage for {paths}"
        )
