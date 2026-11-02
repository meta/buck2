# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

# pyre-strict

import json
from pathlib import Path

from buck2.tests.e2e_util.api.buck import Buck
from buck2.tests.e2e_util.asserts import expect_failure
from buck2.tests.e2e_util.buck_workspace import buck_test, BuckTestCase

REGISTRIES = ["", "prelude//:constraint_override_registry", "root//:registry"]


def _write_python_constraints(root: Path) -> None:
    packages = {
        "os/constraints": """setting(name = "os")
value(name = "linux", setting = ":os")""",
        "build_mode/constraints": """setting(name = "mode")
value(name = "dev", setting = ":mode")
value(name = "opt", setting = ":mode")
setting(name = "link")
value(name = "static", setting = ":link")
value(name = "static_pic", setting = ":link")
group(name = "default_link_style", entries = {"static": ":static", "static_pic": ":static_pic"})
setting(name = "info")
value(name = "full", setting = ":info")
group(name = "fbcode-build-info-mode", entries = {"full": ":full"})
setting(name = "debug")
value(name = "supported", setting = ":debug")
group(name = "native-debugging", entries = {"supported": ":supported"})""",
        "build_mode": """setting(name = "sanitizer")
value(name = "no-san", setting = ":sanitizer")
value(name = "asan-ubsan-dev", setting = ":sanitizer")
group(name = "sanitizer_type", entries = {"no-san": ":no-san", "asan-ubsan-dev": ":asan-ubsan-dev"})
setting(name = "arvr")
value(name = "enabled", setting = ":arvr")
group(name = "arvr_mode", entries = {"enabled": ":enabled"})""",
        "platform/execution/constraints": """setting(name = "execution")
value(name = "execution-platform-transitioned", setting = ":execution")""",
        "build_mode/default_opt_cxx": """setting(name = "opt_cxx")
value(name = "enabled", setting = ":opt_cxx")""",
        "cpu/constraints": """setting(name = "cpu")
value(name = "x86_64", setting = ":cpu")""",
        "toolchain/cxx/constraints": """setting(name = "mcmodel")
value(name = "mcmodel-large", setting = ":mcmodel")""",
    }
    for package, definitions in packages.items():
        path = root / "configuration" / package / "BUCK"
        path.parent.mkdir(parents=True, exist_ok=True)
        definitions = definitions.replace(")\n", ', visibility = ["PUBLIC"])\n')
        if definitions.endswith(")"):
            definitions = definitions[:-1] + ', visibility = ["PUBLIC"])'
        path.write_text(
            'load("@root//:defs.bzl", "setting", "value", "group")\n'
            + definitions
            + "\n"
        )


class ConstraintOverridesTest(BuckTestCase):
    @buck_test(inplace=False)
    async def test_constraint_override_registry_equivalence(self, buck: Buck) -> None:
        expected = {
            "unchanged": ["red", "keep"],
            "constraint": ["blue", "keep"],
            "ordered": ["blue", "keep"],
            "platform": ["blue", "keep"],
            "platform_constraint": ["red", "keep"],
            "subtarget": ["blue", "keep"],
        }
        targets = [f"root//:{name}" for name in expected]
        for registry in REGISTRIES:
            result = await buck.build(
                *targets, "-c", f"buck2.constraint_override_registry={registry}"
            )
            for name, selected in expected.items():
                output = result.get_build_report().output_for_target(f"root//:{name}")
                assert json.loads(output.read_text()) == selected
            query = await buck.cquery(
                "root//:unchanged",
                "-c",
                f"buck2.constraint_override_registry={registry}",
            )
            assert "root//:original#" in query.stdout

    @buck_test(inplace=False)
    async def test_constraint_override_registry_errors_and_invalidation(
        self, buck: Buck
    ) -> None:
        for registry in REGISTRIES:
            config = ["-c", f"buck2.constraint_override_registry={registry}"]
            await expect_failure(
                buck.build("root//:unsupported", *config),
                stderr_regex="Constraint value override not supported: root//:keep",
            )
            await expect_failure(
                buck.build("root//:malformed", *config),
                stderr_regex="Build target must be fully qualified",
            )
            await buck.build("root//:constraint", *config)
            await expect_failure(
                buck.build(
                    "root//:constraint", *config, "-c", "buck2.constraints=root//:red"
                ),
                stderr_regex="Constraint value override not supported: alias//:blue",
            )
            await buck.build("root//:constraint", *config)
            path = buck.cwd / "TARGETS.fixture"
            original = path.read_text()
            try:
                path.write_text(
                    original.replace(
                        'entries = {"blue": ":blue"}', 'entries = {"blue": ":red"}'
                    )
                )
                result = await buck.build("root//:subtarget", *config)
                output = result.get_build_report().output_for_target("root//:subtarget")
                assert json.loads(output.read_text()) == ["red", "keep"]
            finally:
                path.write_text(original)

    @buck_test(inplace=False)
    async def test_legacy_constraint_override_private_visibility(
        self, buck: Buck
    ) -> None:
        path = buck.cwd / "TARGETS.fixture"
        path.write_text(
            path.read_text().replace(
                'value(name = "blue", setting = ":color", visibility = ["PUBLIC"])',
                'value(name = "blue", setting = ":color")',
            )
        )
        await buck.build(
            "root//:constraint", "-c", "buck2.constraint_override_registry="
        )
        await expect_failure(
            buck.build(
                "root//:constraint",
                "-c",
                "buck2.constraint_override_registry=prelude//:constraint_override_registry",
            ),
            stderr_regex="not visible",
        )

    @buck_test(inplace=False)
    async def test_python_constraint_override_registry(self, buck: Buck) -> None:
        _write_python_constraints(buck.cwd)
        config = buck.cwd / ".buckconfig"
        config.write_text(
            config.read_text()
            .replace(
                "  prelude = prelude", "  prelude = prelude\n  config = configuration"
            )
            .replace("ovr_config = prelude", "ovr_config = config")
        )
        targets = buck.cwd / "TARGETS.fixture"
        with targets.open("a") as output:
            output.write("""
load(":defs.bzl", "python_probe")
platform(name = "python_dev", values = ["config//os/constraints:linux", "config//build_mode/constraints:dev", "config//build_mode:sanitizer_type[no-san]"], visibility = ["PUBLIC"])
python_selection = select({"config//build_mode/constraints:dev": ["dev"], "config//build_mode/constraints:opt": ["opt"], "DEFAULT": ["unknown"]})
python_probe(name = "python_unchanged", selected = python_selection)
python_probe(name = "python_opt", selected = python_selection, opt_by_default_enabled = True)
""")
        # @oss-disable[end= ]: expected_opt = ["opt"]
        expected_opt = ["dev"] # @oss-enable
        for registry in REGISTRIES:
            result = await buck.build(
                "root//:python_unchanged",
                "root//:python_opt",
                "--target-platforms",
                "root//:python_dev",
                "-c",
                f"buck2.constraint_override_registry={registry}",
            )
            for name, expected in [
                ("python_unchanged", ["dev"]),
                ("python_opt", expected_opt),
            ]:
                output = result.get_build_report().output_for_target(f"root//:{name}")
                assert json.loads(output.read_text()) == expected
