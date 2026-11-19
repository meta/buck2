# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.


import json
import math
import random
import re
import stat
import string
import sys
import typing
from configparser import ConfigParser
from pathlib import Path

import psutil
from buck2.tests.e2e_util.api.buck import Buck
from buck2.tests.e2e_util.api.buck_result import InvocationRecord


class approx:
    """
    Compares equal to a number within `abs` of `expected`, or within a relative tolerance of
    `rel` (one in a million unless given), so that a float measured by buck2 can sit inside an
    equality assertion over a larger structure, like `pytest.approx`.
    """

    def __init__(
        self,
        expected: float,
        *,
        abs: typing.Optional[float] = None,  # noqa: A002 the name `pytest.approx` has
        rel: typing.Optional[float] = None,
    ) -> None:
        self.expected = expected
        self.abs = abs
        self.rel = rel

    def __eq__(self, actual: object) -> bool:
        if not isinstance(actual, (int, float)):
            return False
        if self.abs is None and self.rel is None:
            return math.isclose(actual, self.expected, rel_tol=1e-6, abs_tol=1e-12)
        return math.isclose(
            actual, self.expected, rel_tol=self.rel or 0.0, abs_tol=self.abs or 0.0
        )

    def __repr__(self) -> str:
        return f"approx({self.expected!r}, abs={self.abs!r}, rel={self.rel!r})"


def daemon_is_alive(pid: int) -> bool:
    return psutil.pid_exists(pid)


def replace_in_file(old: str, new: str, file: Path, encoding: str = "utf-8") -> None:
    """Replace every occurrence of `old` in `file`; `old` must occur at least once."""
    with open(file, encoding=encoding) as f:
        file_content = f.read()
    assert old in file_content, f"{old!r} not found in {file}"
    file_content = file_content.replace(old, new)
    with open(file, "w", encoding=encoding) as f:
        f.write(file_content)


async def read_what_ran(buck: Buck, *args) -> typing.List[typing.Dict[str, typing.Any]]:
    out = await buck.log("what-ran", "--format", "json", *args)
    out = [line.strip() for line in out.stdout.splitlines()]
    out = [json.loads(line) for line in out if line]
    return out


def timestamp_ms(s: int, ns: int) -> int:
    f = int(ns / 1000000)
    assert f < 1000
    return s * 1000 + f


async def read_timestamps(buck: Buck, *args) -> typing.List[int]:
    log = (await buck.log("show")).stdout.strip().splitlines()
    return [
        timestamp_ms(*json.loads(line)["Event"]["timestamp"])
        for line in log
        if json_get(line, *args) is not None
    ]


def remove_output(path: Path) -> None:
    """
    Deletes a build output, which is read-only (`buck2.readonly_outputs`). Unix needs only the
    directory for that; Windows refuses until the attribute is cleared.
    """
    path.chmod(path.stat().st_mode | stat.S_IWRITE)
    path.unlink()


def is_running_on_linux() -> bool:
    return sys.platform == "linux"


def is_running_on_mac() -> bool:
    return sys.platform == "darwin"


def is_running_on_windows() -> bool:
    return sys.platform == "win32"


def get_targets_from_what_ran(
    what_ran: typing.List[typing.Dict[str, typing.Any]],
) -> typing.Set[typing.Tuple[str, str]]:
    targets = set()

    for entry in what_ran:
        m = re.match(r"^(.*?)( \((.*?)\))?( \((.*?)\))?$", entry["identity"])
        assert m is not None
        # `target [(configuration)] (category [identifier])`: the category is the
        # last parenthesised segment, whether or not a configuration precedes it.
        rule = m.group(1)
        category = m.group(5) if m.group(5) is not None else m.group(3)
        targets.add((rule, category))

    return targets


async def expect_exec_count(buck: Buck, n: int) -> None:
    out = await read_what_ran(buck)
    assert len(out) == n, "unexpected actions: %s" % (out,)


async def filter_events(
    buck: Buck,
    *args: str,
    rel_cwd: typing.Optional[Path] = None,
    return_root: bool = False,
) -> typing.List[typing.Any]:
    log = (await buck.log("show", rel_cwd=rel_cwd)).stdout.strip().splitlines()
    found = []
    for line in log:
        e = json_get(line, *args, return_root_on_match=return_root)
        if e is None:
            continue
        found.append(e)
    return found


def json_get(data: str, *key: str, return_root_on_match: bool = False) -> typing.Any:
    root = json.loads(data)
    cur = root

    for k in key:
        cur = cur.get(k)
        if cur is None:
            return None

    return root if return_root_on_match else cur


def random_string() -> str:
    return "".join(random.choice(string.ascii_lowercase) for i in range(256))


def replace_hashes(strings: typing.List[str]) -> typing.List[str]:
    return [replace_hash(s) for s in strings]


def replace_hash(s: str) -> str:
    return re.sub(r"\b[0-9a-f]{16}\b", "<HASH>", s)


def replace_digest(s: str) -> str:
    return re.sub(r"\b[0-9a-f]{40}:[0-9]{1,3}\b", "<DIGEST>", s)


async def get_buck2_re_use_case(buck: Buck) -> str:
    key = "buck2_re_client.override_use_case"
    config = (
        await buck.audit_config("--reuse-current-config", "--style=json", key)
    ).get_json()
    use_case = config.get(key)
    if use_case is not None:
        return use_case

    # The test harness's extra external config is part of normal Buck config
    # parsing, but `override_use_case` is filtered out of DICE-backed config
    # reads, so `audit config --reuse-current-config` may not report it. We need
    # to manually check the test config here.
    extra_config_path = buck.get_env_var("BUCK2_TEST_EXTRA_EXTERNAL_CONFIG")
    if extra_config_path is not None:
        use_case = _get_buck2_re_use_case_from_config_file(Path(extra_config_path))
        if use_case is not None:
            return use_case

    return "buck2-default"


def _get_buck2_re_use_case_from_config_file(path: Path) -> typing.Optional[str]:
    parser = ConfigParser(strict=False)
    if not parser.read(path):
        return None
    return parser.get("buck2_re_client", "override_use_case", fallback=None)


def read_invocation_record(record: Path) -> InvocationRecord:
    return InvocationRecord(record)


def read_streaming_report(path: Path) -> typing.List[typing.Dict[str, typing.Any]]:
    """Parses a `--streaming-build-report` file, one report per line. It may be empty."""
    return [json.loads(line) for line in path.read_text().splitlines()]


def replay_streaming_report(
    reports: typing.List[typing.Dict[str, typing.Any]],
) -> typing.Dict[str, typing.Any]:
    """Merges the `results` of streaming report lines; a key's newest entry wins.

    Works whether each line repeats every entry so far or carries only some of them.
    """
    return {
        key: entry for report in reports for key, entry in report["results"].items()
    }


def streaming_status_history(
    reports: typing.List[typing.Dict[str, typing.Any]],
) -> typing.Dict[str, typing.List[str]]:
    """Each key's successive top-level `success` values across streaming report lines.

    Repeats are collapsed, so the result doesn't depend on how many lines carry an entry.
    """
    history: typing.Dict[str, typing.List[str]] = {}
    for report in reports:
        for key, entry in report["results"].items():
            statuses = history.setdefault(key, [])
            if statuses[-1:] != [entry["success"]]:
                statuses.append(entry["success"])
    return history


async def get_last_execution_kind(
    buck: Buck,
    category: typing.Optional[str] = None,
    excluded_execution_kinds: typing.Optional[typing.List[int]] = None,
    target_name: typing.Optional[str] = None,
) -> typing.Optional[int]:
    if excluded_execution_kinds is None:
        excluded_execution_kinds = []
    action_executions = await filter_events(
        buck,
        "Event",
        "data",
        "SpanEnd",
        "data",
        "ActionExecution",
    )
    for action_execution in reversed(action_executions):
        execution_kind = action_execution.get("execution_kind", None)

        if execution_kind is None or execution_kind in excluded_execution_kinds:
            continue

        if category is not None:
            action_category = action_execution.get("name", {}).get("category", None)
            if action_category != category:
                continue

        if target_name is not None:
            action_target_name = (
                action_execution.get("key", {})
                .get("owner", {})
                .get("TargetLabel", {})
                .get("label", {})
                .get("name", None)
            )
            if action_target_name != target_name:
                continue

        return execution_kind

    return None
