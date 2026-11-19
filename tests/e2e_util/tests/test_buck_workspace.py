# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

import contextlib
import io
import unittest
from pathlib import Path
from typing import Any, AsyncIterator
from unittest import mock

from buck2.tests.e2e_util import buck_workspace
from buck2.tests.e2e_util.api.buck import Buck
from buck2.tests.e2e_util.buck_workspace import (
    buck_test,
    BuckTestCase,
    BuckTestMarker,
    env,
    param,
    parametrize,
    SKIPPABLE_PLATFORMS,
)


def _test_names(cls: type) -> list[str]:
    return sorted(n for n in vars(cls) if n.startswith("test_"))


# The async tests are the inner `Case` classes', run through a runner; this class's are sync.
class ParametrizeTest(unittest.TestCase):  # lint-ignore: AsyncTestInTestCase
    def test_one_case_per_entry_named_after_its_values(self) -> None:
        class Case(BuckTestCase):
            @parametrize("a,b", [(1, "x"), (2, "y z")])
            async def test_pair(self, a: int, b: str) -> None:
                pass

        self.assertEqual(_test_names(Case), ["test_pair_1_x", "test_pair_2_y_z"])

    def test_single_parameter_takes_bare_values(self) -> None:
        class Case(BuckTestCase):
            @parametrize("shape", ["list", ("tuple",)])
            async def test_shape(self, shape: Any) -> None:
                pass

        self.assertEqual(_test_names(Case), ["test_shape_list", "test_shape_shape1"])

    def test_ids_and_param_name_the_cases(self) -> None:
        class Case(BuckTestCase):
            @parametrize("n", [1, 2], ids=["one", "two"])
            async def test_ids(self, n: int) -> None:
                pass

            @parametrize("n", [param(1, id="first"), 2])
            async def test_param(self, n: int) -> None:
                pass

        self.assertEqual(
            _test_names(Case),
            ["test_ids_one", "test_ids_two", "test_param_2", "test_param_first"],
        )

    def test_stacked_decorators_run_the_product_innermost_first(self) -> None:
        class Case(BuckTestCase):
            @parametrize("a", [1, 2])
            @parametrize("b", ["x", "y"])
            async def test_grid(self, a: int, b: str) -> None:
                pass

        self.assertEqual(
            _test_names(Case),
            ["test_grid_x_1", "test_grid_x_2", "test_grid_y_1", "test_grid_y_2"],
        )

    def test_an_empty_parameter_set_leaves_no_case(self) -> None:
        class Case(BuckTestCase):
            @parametrize("n", [])
            async def test_none(self, n: int) -> None:
                raise AssertionError("ran")

            @parametrize("a", [1, 2])
            @parametrize("b", [])
            async def test_empty_factor(self, a: int, b: int) -> None:
                raise AssertionError("ran")

            async def test_other(self) -> None:
                pass

        result = unittest.TextTestRunner(stream=io.StringIO()).run(
            unittest.defaultTestLoader.loadTestsFromTestCase(Case)
        )
        self.assertEqual(_test_names(Case), ["test_other"])
        self.assertEqual((result.testsRun, result.skipped), (1, []))

    def test_a_subclass_redefining_a_case_replaces_it(self) -> None:
        class Base(BuckTestCase):
            @parametrize("n", [1])
            async def test_it(self, n: int) -> None:
                pass

        class Derived(Base):
            @parametrize("n", [1])
            async def test_it(self, n: int) -> None:
                pass

        self.assertEqual(_test_names(Derived), ["test_it_1"])

    def test_a_sync_wrapper_hiding_a_coroutine_is_an_error(self) -> None:
        class Case(BuckTestCase):
            @parametrize("n", [1])
            def test_it(self, n: int) -> Any:
                async def body() -> None:
                    pass

                return body()

        result = unittest.TextTestRunner(stream=io.StringIO()).run(
            unittest.defaultTestLoader.loadTestsFromTestCase(Case)
        )
        self.assertEqual(len(result.errors), 1)
        self.assertIn("make it async", result.errors[0][1])

    def test_colliding_names_are_numbered(self) -> None:
        class Case(BuckTestCase):
            @parametrize("s", ["a b", "a-b"])
            async def test_dup(self, s: str) -> None:
                pass

        self.assertEqual(_test_names(Case), ["test_dup_a_b", "test_dup_a_b_2"])

    def test_cases_receive_their_arguments(self) -> None:
        seen = []

        class Case(BuckTestCase):
            @parametrize("a,b", [(1, 2)])
            async def test_args(self, a: int, b: int) -> None:
                seen.append((a, b))

        unittest.TextTestRunner(stream=io.StringIO()).run(
            unittest.defaultTestLoader.loadTestsFromTestCase(Case)
        )
        self.assertEqual(seen, [(1, 2)])


# The async tests are the inner `Case` classes', run through a runner; this class's are sync.
class BuckTestMethodTest(unittest.TestCase):  # lint-ignore: AsyncTestInTestCase
    def run_case(self, cls: type) -> unittest.TestResult:
        fake_buck = mock.Mock(spec=Buck)
        self.fixture_args: dict[str, Any] = {}

        @contextlib.asynccontextmanager
        async def fake_fixture(
            marker: BuckTestMarker, test_id: str | None = None
        ) -> AsyncIterator[Buck]:
            self.fixture_args["marker"] = marker
            self.fixture_args["test_id"] = test_id
            yield fake_buck

        with mock.patch.object(buck_workspace, "buck_fixture", fake_fixture):
            result = unittest.TextTestRunner(stream=io.StringIO()).run(
                unittest.defaultTestLoader.loadTestsFromTestCase(cls)
            )
        self.assertEqual(result.errors, [])
        return result

    def test_method_gets_a_buck_and_a_tmp_path(self) -> None:
        seen: dict[str, Any] = {}

        class Case(BuckTestCase):
            @buck_test(inplace=True)
            async def test_it(self, buck: Buck, tmp_path: Path) -> None:
                seen["buck"] = buck
                seen["tmp_path_is_dir"] = tmp_path.is_dir()

        result = self.run_case(Case)
        self.assertEqual(result.failures, [])
        self.assertIsInstance(seen["buck"], Buck)
        self.assertTrue(seen["tmp_path_is_dir"])
        self.assertTrue(self.fixture_args["test_id"].endswith("Case.test_it"))
        self.assertTrue(self.fixture_args["marker"].inplace)

    def test_env_and_parametrize_apply_to_a_method(self) -> None:
        seen = []

        class Case(BuckTestCase):
            @buck_test(inplace=True)
            @env("KEY", "value")
            @parametrize("n", [1])
            async def test_it(self, buck: Buck, n: int) -> None:
                seen.append(n)
                set_env = buck.set_env
                assert isinstance(set_env, mock.Mock)
                set_env.assert_called_once_with("KEY", "value")

        result = self.run_case(Case)
        self.assertEqual(result.failures, [])
        self.assertEqual(_test_names(Case), ["test_it_1"])
        self.assertEqual(seen, [1])

    def test_a_test_without_a_buck_parameter_starts_no_daemon(self) -> None:
        seen = []

        class Case(BuckTestCase):
            @buck_test(inplace=True)
            async def test_it(self) -> None:
                seen.append(self.id())

        result = self.run_case(Case)
        self.assertEqual(result.failures, [])
        self.assertEqual(len(seen), 1)
        self.assertNotIn("test_id", self.fixture_args)

    def test_a_sync_test_without_fixtures_stays_a_plain_method(self) -> None:
        seen = []

        class Case(unittest.TestCase):
            @buck_test(inplace=True)
            def test_it(self) -> None:
                seen.append(self.id())

            @buck_test(inplace=True, skip_for_os=SKIPPABLE_PLATFORMS)
            def test_skipped(self) -> None:
                raise AssertionError("ran")

        result = unittest.TextTestRunner(stream=io.StringIO()).run(
            unittest.defaultTestLoader.loadTestsFromTestCase(Case)
        )
        self.assertEqual((result.testsRun, result.errors, result.failures), (2, [], []))
        self.assertEqual(len(seen), 1)

    def test_a_sync_test_runs(self) -> None:
        seen = []

        class Case(BuckTestCase):
            @buck_test(inplace=True)
            def test_it(self, buck: Buck) -> None:
                seen.append(buck)

        self.run_case(Case)
        self.assertEqual(len(seen), 1)

    def test_a_failing_test_fails(self) -> None:
        class Case(BuckTestCase):
            @buck_test(inplace=True)
            async def test_it(self, buck: Buck) -> None:
                raise AssertionError("expected")

        result = self.run_case(Case)
        self.assertEqual(len(result.failures), 1)

    def test_skipped_os_passes_without_a_buck(self) -> None:
        with mock.patch.object(buck_workspace.platform, "system", return_value="Linux"):

            class Case(BuckTestCase):
                @buck_test(inplace=True, skip_for_os=["linux"])
                @parametrize("n", [1, 2])
                async def test_it(self, buck: Buck, n: int) -> None:
                    raise AssertionError("ran")

        result = self.run_case(Case)
        self.assertEqual(_test_names(Case), ["test_it_1", "test_it_2"])
        self.assertEqual(result.testsRun, 2)
        self.assertEqual(result.failures, [])
