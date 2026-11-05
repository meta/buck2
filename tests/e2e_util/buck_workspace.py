#!/usr/bin/env fbpython
# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

# pyre-strict

import asyncio
import contextlib
import functools
import hashlib
import inspect
import itertools
import json
import os
import platform
import re
import shutil
import stat
import subprocess
import sys
import tempfile
import time
import unittest
from collections import namedtuple
from pathlib import Path
from typing import (
    Any,
    AsyncGenerator,
    Awaitable,
    Callable,
    Dict,
    Iterable,
    List,
    Optional,
    Sequence,
)

import __manifest__
from buck2.tests.e2e_util.api.buck import Buck
from buck2.tests.e2e_util.api.buck_result import BuckException
from buck2.tests.e2e_util.api.executable import WindowsCmdOption
from decorator import decorator

if sys.platform != "win32":
    import fcntl

BuckTestMarker = namedtuple(
    "BuckTestMarker",
    [
        "inplace",
        "data_dir",
        "allow_soft_errors",
        "extra_buck_config",
        "skip_final_kill",
        "setup_eden",
        "disable_daemon_cgroup",
        "write_invocation_record",
        "allow_daemon_panics",
    ],
)


def _is_core_test() -> bool:
    """Whether the running test is one of the `tests/core` tests."""
    return os.environ.get("BUCK2_E2E_TEST_FLAVOR") == "isolated"


@contextlib.asynccontextmanager
async def buck_fixture(  # noqa C901 : "too complex"
    marker: BuckTestMarker,
    test_id: str,
) -> AsyncGenerator[Buck, None]:
    """
    Returns a Buck for testing.

    `test_id` names the test case (`TestCase.id()`); an inplace test's isolation prefix is
    derived from it, so it must differ between the test cases of one target.
    """

    is_windows = platform.system() == "Windows"
    test_executable = os.environ["TEST_EXECUTABLE"]

    env: Dict[str, str] = {**os.environ}
    env["BUCK2_SHARE_DAEMON"] = "false"
    env.pop("BUCK2_DAEMON_GROUP", None)
    # This is necessary for static linking on Linux.
    if platform.system() != "Windows":
        env["BUCKD_STARTUP_TIMEOUT"] = "120"
        env["BUCKD_STARTUP_INIT_TIMEOUT"] = "120"
    else:
        env["CI"] = "true"

    # allow_soft_errors will override any existing environment variable behavior
    if marker.allow_soft_errors or marker.inplace:
        env["BUCK2_HARD_ERROR"] = "false"

    # Use a very small stdin buffer to catch any scenarios in which we
    # don't properly handle partial input.
    env["BUCK2_TEST_STDIN_BUFFER_SIZE"] = "8"
    # Explicitly disable log uploading, we don't care about stats for tests.
    env["BUCK2_TEST_DISABLE_LOG_UPLOAD"] = "true"
    # But still block on it, because the upload process also writes
    # locally, and we want that to be synchronous instead of backgrounded.
    env["BUCK2_TEST_BLOCK_ON_UPLOAD"] = "true"
    # Require the events dispatcher to be set for e2e tests.
    env["ENFORCE_DISPATCHER_SET"] = "true"
    # Inform buck of the test timeout so the daemon self-terminates (with its
    # +30s grace, see self_test_timeout.rs) before TPX hard-kills the test.
    # TPX publishes the real per-test budget in TPX_TIMEOUT_SEC, which already
    # reflects timeout labels like `long_running` (10x) and `glacial` (16.5x).
    # Honor that budget, minus headroom for the grace and final cleanup, so a
    # labeled test's inner buck2 actually gets its extended runway instead of
    # being capped at ~630s. Never drop below the historical 600s floor.
    tpx_timeout_s = os.environ.get("TPX_TIMEOUT_SEC")
    self_test_timeout_s = 600
    if tpx_timeout_s and tpx_timeout_s.isdigit():
        self_test_timeout_s = max(self_test_timeout_s, int(tpx_timeout_s) - 90)
    env["BUCK2_SELF_TEST_TIMEOUT_S"] = str(self_test_timeout_s)
    # Timeout Watchman requests because we often see it hang and crash.
    env["BUCK2_WATCHMAN_TIMEOUT"] = "30"
    env["BUCK2_RUNTIME_THREADS"] = "8"
    # Avoid noise in stderr.
    env["BUCK2_IGNORE_VERSION_EXTRACTION_FAILURE"] = "true"
    env["SUPERCONSOLE_TESTING_WIDTH"] = "100"
    env["SUPERCONSOLE_TESTING_HEIGHT"] = "100"
    # Don't try to assign to a new cgroup during tests.
    if marker.disable_daemon_cgroup:
        env["BUCK2_TEST_DISABLE_DAEMON_CGROUP"] = "true"

    assert "BUCK2_RUNTIME_THREADS" in env, (
        "BUCK2_RUNTIME_THREADS should be set by the test macros"
    )
    assert "BUCK2_MAX_BLOCKING_THREADS" in env, (
        "BUCK2_MAX_BLOCKING_THREADS should be set by the test macros"
    )
    # Windows uses blocking threads for subprocess I/O so we can't do this there.
    del env["BUCK2_MAX_BLOCKING_THREADS"]

    # Filter out some environment variables that may interfere with the
    # running of tests. Notably, since this framework is used to write
    # Python tests that run Buck, we clear out Python test environment
    # variables so that if we run a Python test via Buck 2, they won't
    # interfere.
    env.pop("TEST_PILOT", None)

    common_dir = await _get_common_dir()
    base_dir = Path(tempfile.mkdtemp(dir=common_dir))

    isolation_prefix = None
    keep_temp = os.environ.get("BUCK_E2E_KEEP_TEMP") == "1"

    env["BUCK2_TEST_SKIP_DEFAULT_EXTERNAL_CONFIG"] = "true"

    # Because we may change the working directory, create an absolute path to the test data srcs if
    # the exist and make it available in a different envvar. This is used by golden tests
    test_repo_data = os.environ.get("TEST_REPO_DATA")
    if test_repo_data is not None:
        os.environ["TEST_REPO_DATA_SRC"] = str(Path(test_repo_data).absolute())

    # Create a temporary file to store all lines of extra buck config values.
    extra_config_lines = []

    # Core tests run all their RE actions under `buck2-testing`, which has an
    # isolated CAS namespace (cas_store_version offset by TEST_OFFSET=200). Its
    # quota is sized for their small actions only; the other users of this
    # harness build real targets and stay on the use case their execution
    # platforms declare.
    if _is_core_test():
        extra_config_lines.append(
            "[buck2_re_client]\noverride_use_case = buck2-testing\n"
        )

    project_dir = base_dir / "project"

    # Temp dir needed for EdenFS, will only be created if necessary
    eden_dir = base_dir / "eden"

    orig_stdout = sys.stdout
    try:
        # Redirect stdout to stderr during the test so that `print` statements
        # show up in the test failure output by default for debugging.
        sys.stdout = sys.stderr

        if sys.platform == "darwin":
            _ensure_fseventsd_running(common_dir)

        if marker.setup_eden:
            assert not marker.inplace, (
                "EdenFS for e2e tests is not supported for inplace tests"
            )

            _setup_eden(
                eden_dir,
                project_dir,
                env,
                is_windows,
            )

        if marker.inplace:
            # We need a unique isolated prefix per test case.
            current_test = _test_binary_identity() + test_id
            isolation_prefix = hashlib.sha256(current_test.encode("utf-8")).hexdigest()[
                :40
            ]
            # FIXME(T136079642): Buck2 on Windows has problem with relative symlinks over 260 chars, shorten the hash
            if is_windows:
                isolation_prefix = isolation_prefix[:5]
            else:
                isolation_prefix = ".buck_e2e" + isolation_prefix

            buck_cwd = Path.cwd()

            # NOTE: In theory, this isn't true of all Linux hosts, but all
            # our tests actually rely on it and will break if you ran them
            # on a host without this, so just make it the default.
            if sys.platform == "linux":
                extra_config_lines.append("[host_features]\ngvfs = true\n")
            extra_config_lines.append("[buildfile]\nextra_for_test = TARGETS.test\n")

        else:
            if marker.data_dir is not None:
                src = Path(os.environ["TEST_REPO_DATA"], marker.data_dir)
                _copytree(src, project_dir)
                _maybe_setup_prelude_and_ovr_config(project_dir)
                if _is_core_test():
                    _check_core_test_cells(project_dir)
                with open(Path(project_dir, ".watchmanconfig"), "w") as f:
                    # Use the FS Events watcher, which is more reliable than the default.
                    json.dump(
                        {
                            "ignore_dirs": ["buck-out", ".hg"],
                            "fsevents_watch_files": True,
                            "prefer_split_fsevents_watcher": False,
                        },
                        f,
                    )

            # `edenfs` watcher requires eden to be setup which is too slow to enable on all tests
            # use edenfs watcher whenever possible in test, otherwise use `fs_hash_crawler`
            # FYI: if you remove this, make sure to remove it from external_buckconfig tests too
            if marker.setup_eden:
                extra_config_lines.append("[buck2]\nfile_watcher = edenfs\n")
                extra_config_lines.append("[buck2]\nallow_eden_io = true\n")
            else:
                extra_config_lines.append("[buck2]\nfile_watcher = fs_hash_crawler\n")

            buck_cwd = project_dir

        # Core tests run with read-only outputs, under either materializer.
        extra_config_lines.append("[buck2]\nreadonly_outputs = true\n")
        # A target variant can run its suite against another materializer implementation;
        # see `buck2_core_tests`.
        materializer = os.environ.get("BUCK2_TEST_MATERIALIZER")
        if materializer == "v2":
            # Every `ROLLOUT_VERSION` is at least 0.
            extra_config_lines.append("[buck2]\nmaterializer_v2_version = 0\n")
            # The disk-state twins run strict: a root leaves disk the moment its last lease
            # does, so a reader without one fails here rather than by chance in use.
            extra_config_lines.append("[buck2]\nmaterializer_strict_leases = true\n")
        elif materializer is not None:
            raise Exception(f"Unknown `BUCK2_TEST_MATERIALIZER` value `{materializer}`")

        for section, config in marker.extra_buck_config.items():
            extra_config_lines.append(f"[{section}]\n")
            for key, value in config.items():
                extra_config_lines.append(f"{key} = {value}\n")

        extra_config = os.path.join(base_dir, "extra.bcfg")
        with open(extra_config, "w") as f:
            for line in extra_config_lines:
                f.write(line)
        env["BUCK2_TEST_EXTRA_EXTERNAL_CONFIG"] = extra_config

        settings_home_dir = os.path.join(base_dir, "settings_home")
        os.makedirs(settings_home_dir, exist_ok=True)
        env["BUCK2_TEST_SETTINGS_HOME_DIR"] = settings_home_dir

        buck = Buck(
            Path(test_executable),
            cwd=buck_cwd,
            encoding="utf-8",
            env=env,
            write_invocation_record=marker.write_invocation_record,
        )

        if isolation_prefix is not None:
            buck.set_isolation_prefix(isolation_prefix)

        yield buck

        daemon_panic = None
        if not (marker.allow_daemon_panics or marker.inplace):
            if not marker.skip_final_kill:
                # Kill (not clean) first: it waits for the daemon to exit and leaves its
                # stderr in place for the check.
                await buck.kill()
            daemon_panic = await _daemon_panic(buck)

        if not marker.skip_final_kill:
            if keep_temp:
                await buck.kill()
            else:
                await buck.clean()

        if daemon_panic is not None:
            raise AssertionError(daemon_panic)
    finally:
        sys.stdout = orig_stdout

        if keep_temp:
            print(f"Not deleting temporary directory at {base_dir}", file=sys.stderr)
        else:
            if marker.setup_eden:
                _cleanup_eden(eden_dir, project_dir, env)
            shutil.rmtree(base_dir, ignore_errors=True)


def _test_binary_identity() -> str:
    """
    What tells this test binary apart from the others: its target, or, where the build does not
    record one (the fbsource cell), the path it runs from. A template module shared by many
    targets (`bxl_test`'s) gives every target the same test ids.
    """
    return __manifest__.fbmake["build_rule"] or os.path.realpath(sys.argv[0])


async def _get_common_dir() -> Path:
    from asyncio import subprocess

    """
    Returns a temporary directory using mkscratch.
    The advantage of using mkscratch is that it can return the same directory on multiple calls.
    If mkscratch is not available (e.g., Windows or remote environments), fall back to the system temp directory.
    """
    # Check if mkscratch is available, fall back to tempfile.gettempdir() if not
    if shutil.which("mkscratch") is None:
        return Path(tempfile.gettempdir())

    # Need to use `--hash` over `--subdir` here because the tmp path would be too long and
    # Eden would fail with `Socket path too large to fit into sockaddr_un` otherwise
    mkscratch_proc = await subprocess.create_subprocess_exec(
        "mkscratch",
        "path",
        "--hash",
        stdout=subprocess.PIPE,
    )
    stdout, _ = await mkscratch_proc.communicate()
    assert stdout is not None, "stdout should not be None"
    common_dir = Path(stdout.decode().strip())
    return common_dir


async def _daemon_panic(buck: Buck) -> Optional[str]:
    """
    A failure message if any daemon this test used panicked, else None.

    A daemon that panics after answering a command goes unnoticed: the client already has its
    result and the next command starts a new daemon. So this reads the daemon's stderr after
    the test, including the previous daemon's copy in `prev/`.
    """
    if not buck.cwd.is_dir():
        # The test deleted its own project.
        return None
    try:
        daemon_dir = await buck.get_daemon_dir()
    except BuckException:
        # No project root, so no daemon.
        return None
    project_daemon_dir = daemon_dir.parent
    if not project_daemon_dir.is_dir():
        return None
    for stderr in sorted(project_daemon_dir.glob("*/buckd.stderr")) + sorted(
        project_daemon_dir.glob("*/prev/buckd.stderr")
    ):
        try:
            text = stderr.read_text(errors="replace")
        except OSError:
            continue
        if "panicked at" not in text:
            continue
        if _HARD_ERROR_UPGRADE in text:
            what = (
                "The buck2 daemon hit a soft error during this test. Tests run with "
                "BUCK2_HARD_ERROR, which turns soft errors into panics. If this soft error is "
                "expected, mark the test `allow_soft_errors=True` (or `allow_daemon_panics=True` "
                "to keep hard errors for everything else)."
            )
        else:
            what = (
                "The buck2 daemon panicked during this test. If that is expected, mark the test "
                "`allow_daemon_panics=True`."
            )
        return f"{what} Daemon stderr ({stderr}):\n\n{text}"
    return None


_HARD_ERROR_UPGRADE = "Upgraded warning to failure via $BUCK2_HARD_ERROR"


def _buck_argument(args: Sequence[Any], kwargs: Dict[str, Any]) -> Buck:
    """The `Buck` a decorated test was called with: the first argument of a test function,
    the one after `self` of a test method."""
    for arg in itertools.chain(args, kwargs.values()):
        if isinstance(arg, Buck):
            return arg
    raise TypeError("test was called without a `Buck`")


def nobuckd(fn: Callable[..., Any]) -> Callable[..., Any]:
    """Disables buck daemon"""

    def wrapped(fn: Callable[..., Any], *args: Any, **kwargs: Any) -> Any:
        _buck_argument(args, kwargs).set_buckd(True)
        return fn(*args, **kwargs)

    return decorator(wrapped, fn)


def _eden_base_cmd(eden_dir: Path) -> List[str]:
    config_dir = eden_dir / "config"
    etc_dir = eden_dir / "etc"
    home_dir = eden_dir / "home"

    config_dir.mkdir(exist_ok=True)
    etc_dir.mkdir(exist_ok=True)
    home_dir.mkdir(exist_ok=True)

    return [
        "eden",
        "--config-dir",
        str(config_dir),
        "--home-dir",
        str(home_dir),
        "--etc-eden-dir",
        str(etc_dir),
    ]


# Adapted from Eden integration test, didn't use their code because Eden uses the compiled binary in their buck-out
# which we don't have, extracting that part out would be more work than what was done below.
# https://www.internalfb.com/code/fbsource/[45334ead4a72]/fbcode/eden/integration/lib/testcase.py?lines=123
def _setup_eden(
    eden_dir: Path,
    project_dir: Path,
    env: Dict[str, str],
    is_windows: bool,
) -> None:
    eden_dir.mkdir(exist_ok=True)
    # Start up an EdenFS Client and point it to the temp dirs
    subprocess.check_call(
        _eden_base_cmd(eden_dir)
        + [
            "start",
        ],
        stdout=sys.stdout,
        stderr=sys.stderr,
        env=env,
    )

    temp_repo = eden_dir / "temp_repo"
    # Initialize a hg repo, so Eden can mount it
    subprocess.check_call(
        ["hg", "init", str(temp_repo)],
        stdout=sys.stdout,
        stderr=sys.stderr,
        env=env,
    )

    # Use .eden-redirections to force redirection to be setup at mount time
    # The number of concurrent APFS volumes we can create on macOS
    # is limited. Furthermore, cleaning up disk image redirections is non-trivial.
    # Let's use symlink redirections to avoid these issues.
    redirection_type = "symlink" if sys.platform == "darwin" else "bind"
    with open(temp_repo / ".eden-redirections", "w") as f:
        f.write(f'[redirections]\n"buck-out" = "{redirection_type}"\n')

    # Make sure the repo's `sl status` is clean. Eden auto-creates the redirections
    with open(temp_repo / ".gitignore", "w") as f:
        f.write("/buck-out\n")

    subprocess.check_call(
        ["hg", "commit", "--addremove", "-m", "init"],
        stdout=sys.stdout,
        stderr=sys.stderr,
        env=env,
        cwd=temp_repo,
    )

    # Mount the hg repo we created
    project_dir.mkdir(exist_ok=True)
    cmd = _eden_base_cmd(eden_dir) + [
        "clone",
        temp_repo,
        project_dir,
        "--allow-empty-repo",
        "--case-insensitive",
    ]

    if is_windows:
        cmd.append("--enable-windows-symlinks")

    subprocess.check_call(
        cmd,
        stdout=sys.stdout,
        stderr=sys.stderr,
        env=env,
    )


def eden_restart(
    eden_dir: Path,
    env: Dict[str, str],
) -> None:
    """Gracefully restart the EdenFS daemon (the mount survives via takeover, but the
    daemon process changes)."""
    subprocess.check_call(
        _eden_base_cmd(eden_dir)
        + [
            "restart",
            "--graceful",
        ],
        stdout=sys.stdout,
        stderr=sys.stderr,
        env=env,
    )


def eden_remove(
    eden_dir: Path,
    project_dir: Path,
    env: Dict[str, str],
) -> None:
    """Remove an Eden checkout mount."""
    subprocess.run(
        _eden_base_cmd(eden_dir)
        + [
            "remove",
            str(project_dir),
            "-y",
        ],
        stdout=sys.stdout,
        stderr=sys.stderr,
        env=env,
    )


def _cleanup_eden(
    eden_dir: Path,
    project_dir: Path,
    env: Dict[str, str],
) -> None:
    eden_remove(eden_dir, project_dir, env)

    subprocess.run(
        _eden_base_cmd(eden_dir)
        + [
            "shutdown",
        ],
        stdout=sys.stdout,
        stderr=sys.stderr,
        env=env,
    )


def _copy_writable(src: str | os.PathLike[str], dst: str | os.PathLike[str]) -> None:
    """Copies a file, giving the copy its owner's write permission; the test
    writes to its copy of fixtures that buck materializes read-only."""
    shutil.copy2(src, dst)
    os.chmod(dst, os.stat(dst).st_mode | stat.S_IWUSR)


def _copytree(
    src: Path,
    dst: Path,
    symlinks: bool = False,
    ignore: Optional[Callable[..., Iterable[str]]] = None,
) -> None:
    """Copies all files and directories from src into dst"""
    dst.mkdir(parents=True, exist_ok=True)
    for item in os.listdir(src):
        if item == "buck-out":
            continue
        s = src / item
        d = dst / item
        if os.path.isdir(s):
            shutil.copytree(
                s, d, symlinks, ignore, copy_function=_copy_writable, dirs_exist_ok=True
            )
        else:
            _copy_writable(s, d)


def _check_core_test_cells(project_dir: Path) -> None:
    """Rejects `tests/core` fixtures that use the bundled prelude.

    Those tests are of buck2 itself, so the only bundled cell they get is the nano prelude.
    Anything that needs the real prelude belongs in `tests/e2e`.
    """
    if os.environ.get("BUCK2_E2E_TEST_ALLOW_BUNDLED_CELLS"):
        return
    for buckconfig in project_dir.rglob(".buckconfig"):
        section = None
        for line in buckconfig.read_text().splitlines():
            line = line.split("#", 1)[0].strip()
            if line.startswith("["):
                section = line
            elif section == "[external_cells]" and "=" in line:
                cell, _, kind = (part.strip() for part in line.partition("="))
                if kind == "bundled" and cell != "nano_prelude":
                    raise Exception(
                        f"`{buckconfig.relative_to(project_dir)}` sets `{cell} = bundled`, but "
                        "`nano_prelude` is the only bundled cell `tests/core` tests may use: they "
                        "test buck2 itself, not the prelude. Tests that need prelude code belong "
                        "in `tests/e2e`; see `tests/README.md`."
                    )


_FSEVENTSD_KICKSTART = "/usr/local/bin/kickstart-fseventsd"
_fseventsd_ready = False


def _ensure_fseventsd_running(lock_dir: Path) -> None:
    """
    Sandcastle Mac VMs boot with fseventsd disabled and ship a sudo-whitelisted
    script that starts it.
    """
    global _fseventsd_ready
    if _fseventsd_ready:
        return
    try:
        _start_fseventsd(lock_dir)
    except (OSError, subprocess.SubprocessError) as e:
        # Reported here and retried by the next test in this process.
        print(f"could not start fseventsd: {e}", file=sys.stderr)
        return
    _fseventsd_ready = True


def _start_fseventsd(lock_dir: Path) -> None:
    run = functools.partial(
        subprocess.run, timeout=30, stdout=sys.stdout, stderr=sys.stderr
    )

    def running(process: str) -> bool:
        return run(["/usr/bin/pgrep", "-qx", process]).returncode == 0

    def wait_until(cond: Callable[[], bool], failure: str) -> None:
        for _ in range(300):
            if cond():
                return
            time.sleep(0.1)
        raise TimeoutError(failure)

    # Test processes run in parallel. The lock serializes the watchman restart
    # below, which is per user like the daemon; the kick itself is idempotent.
    with open(lock_dir / "fseventsd.lock", "w") as lock:
        fcntl.flock(lock, fcntl.LOCK_EX)
        if running("fseventsd"):
            return
        if not os.access(_FSEVENTSD_KICKSTART, os.X_OK):
            raise FileNotFoundError(
                f"fseventsd is not running and {_FSEVENTSD_KICKSTART} is missing"
            )
        run(["/usr/bin/sudo", "-n", _FSEVENTSD_KICKSTART], check=True)
        wait_until(lambda: running("fseventsd"), "fseventsd did not start")
        # A daemon that failed to watch while fseventsd was down may keep
        # failing, and a client that spawns a daemon while the old one is still
        # shutting down times out on its pid lock, so replace the daemon here.
        watchman = shutil.which("watchman") or "/usr/local/bin/watchman"
        if run([watchman, "--no-spawn", "shutdown-server"]).returncode != 0:
            run(["/usr/bin/pkill", "-x", "watchman"])
        wait_until(
            lambda: not running("watchman"), "the old watchman daemon did not exit"
        )
        run([watchman, "version"], check=True)


def _maybe_setup_prelude_and_ovr_config(path: Path) -> None:
    if "PRELUDE" in os.environ or "OVR_CONFIG" in os.environ:
        if _is_core_test():
            raise Exception(
                "Don't set `PRELUDE` or `OVR_CONFIG` in `tests/core` - these tests are always isolated"
            )

    if "PRELUDE" in os.environ:
        prelude = Path(path, "prelude")
        if not prelude.exists():
            _copytree(
                Path(os.environ["PRELUDE"]),
                Path(path, "prelude"),
            )

    # TODO: The toolchain platform definitions we hard-code in the prelude are
    # in the ovr_config cell, so copy them in for now.  Longer-term, D31566140
    # has a discusion on bettter approaches.
    if "OVR_CONFIG" not in os.environ:
        return
    _copytree(
        Path(os.pardir, "arvr", "tools", "build_defs", "config"),
        Path(path, "arvr", "tools", "build_defs", "config"),
    )

    _copytree(
        Path(os.pardir, "tools", "build_defs", "fbcode_macros"),
        Path(path, "tools", "build_defs", "fbcode_macros"),
    )

    with Path(path, ".buckconfig").open("a") as f:
        print(
            "", file=f
        )  # append newline because test `.buckconfig` may not end with newline
        print("# Following lines are added by buck_workspace.py", file=f)
        print("[repositories]", file=f)
        print("ovr_config = arvr/tools/build_defs/config", file=f)
        print("fbcode_macros = tools/build_defs/fbcode_macros", file=f)
        print("config = arvr/tools/build_defs/config", file=f)
    with Path(path, "arvr", "tools", "build_defs", "config", ".buckconfig").open(
        "w"
    ) as f:
        pass
    with Path(path, "tools", "build_defs", "fbcode_macros", ".buckconfig").open(
        "w"
    ) as f:
        pass


BuckTestFn = Callable[..., Awaitable[None]]

SKIPPABLE_PLATFORMS = ["darwin", "linux", "windows"]


_FIXTURE_PARAMETERS = ("self", "buck", "tmp_path")


def _is_test_method(fn: Callable[..., Any]) -> bool:
    return next(iter(inspect.signature(fn).parameters), None) == "self"


def _passing_test_method(fn: Callable[..., Any]) -> Callable[..., Any]:
    @functools.wraps(fn)
    def always_pass(self: unittest.TestCase, *args: Any, **kwargs: Any) -> None:
        pass

    return always_pass


def _buck_test_method(
    marker: BuckTestMarker, fn: Callable[..., Any]
) -> Callable[..., Any]:
    """
    `fn` as a test method of a `BuckTestCase`: it runs with a `Buck` for its `buck` parameter
    and a fresh directory for `tmp_path`, whichever of the two it has (a test without `buck`
    starts no daemon). Any further parameters are the test's `parametrize` arguments.
    """
    parameters = list(inspect.signature(fn).parameters)
    wants_buck = "buck" in parameters
    wants_tmp_path = "tmp_path" in parameters
    positional = [p for p in parameters if p not in _FIXTURE_PARAMETERS]
    if not (wants_buck or wants_tmp_path or inspect.iscoroutinefunction(fn)):
        # Nothing to supply: a plain method, which a plain `unittest.TestCase` can hold.
        return fn

    @functools.wraps(fn)
    async def test(self: "BuckTestCase", *args: Any, **kwargs: Any) -> None:
        kwargs.update(zip(positional, args))
        async with contextlib.AsyncExitStack() as stack:
            if wants_buck:
                kwargs["buck"] = await stack.enter_async_context(
                    buck_fixture(marker, test_id=self.id())
                )
            if wants_tmp_path:
                kwargs["tmp_path"] = Path(
                    stack.enter_context(
                        tempfile.TemporaryDirectory(ignore_cleanup_errors=True)
                    )
                )
            result = fn(self, **kwargs)
            if inspect.isawaitable(result):
                await result

    return test


class BuckTestCase(unittest.IsolatedAsyncioTestCase):
    """
    The base class of a buck e2e test module. Each test is an `async def test_*(self, buck:
    Buck, ...)` method decorated with `buck_test()`, which supplies the `buck` (and, when the
    method takes one, a `tmp_path: Path`). `parametrize()` runs a method once per set of
    arguments.
    """

    def __init_subclass__(cls, **kwargs: Any) -> None:
        super().__init_subclass__(**kwargs)
        _expand_parametrized_tests(cls)

    async def asyncSetUp(self) -> None:
        await super().asyncSetUp()
        # The debug mode `IsolatedAsyncioTestCase` enables logs a warning for every await of a
        # buck command that takes longer than 100ms.
        asyncio.get_running_loop().set_debug(False)


class param:
    """One set of `parametrize()` arguments with a name of its own."""

    __slots__ = ("values", "id")

    def __init__(self, *values: Any, id: Optional[str] = None) -> None:
        self.values = values
        self.id = id


_PARAMETRIZE_ATTR = "_buck_parametrize"


def parametrize(
    argnames: str | Sequence[str],
    argvalues: Iterable[Any],
    ids: Optional[Iterable[Optional[str]] | Callable[[Any], Optional[str]]] = None,
) -> Callable[[Callable[..., Any]], Callable[..., Any]]:
    """
    Runs the decorated test method once per entry of `argvalues`, as its own test case. It
    takes what pytest's `parametrize` took: `argnames` is a comma-separated string or a
    sequence of the parameters to bind, each entry of `argvalues` their values (a single value
    for a single parameter), `ids` names the cases. Stacked `parametrize` decorators run the
    product of their entries.

    The test cases are named after the test and the case: `test_foo` with the id `linux` is
    `test_foo_linux`. An empty `argvalues` leaves no case: the test does not exist.
    """
    names = (
        [n.strip() for n in argnames.split(",")]
        if isinstance(argnames, str)
        else list(argnames)
    )
    ids_list = list(ids) if ids is not None and not callable(ids) else None
    cases = []
    for index, entry in enumerate(argvalues):
        if isinstance(entry, param):
            values, case_id = entry.values, entry.id
        else:
            values, case_id = (entry if len(names) > 1 else (entry,)), None
        if len(values) != len(names):
            raise ValueError(
                f"parametrize: {len(names)} names {names} but {len(values)} values {values!r}"
            )
        if case_id is None and ids_list is not None:
            case_id = ids_list[index]
        if case_id is None and callable(ids):
            case_id = "-".join(
                ids(v) or _default_id(n, v, index) for n, v in zip(names, values)
            )
        if case_id is None:
            case_id = "-".join(_default_id(n, v, index) for n, v in zip(names, values))
        cases.append((case_id, dict(zip(names, values))))

    def decorate(fn: Callable[..., Any]) -> Callable[..., Any]:
        # Decorators apply innermost first, so the innermost `parametrize` leads the ids and
        # varies slowest, as it did under pytest.
        setattr(fn, _PARAMETRIZE_ATTR, getattr(fn, _PARAMETRIZE_ATTR, []) + [cases])
        return fn

    return decorate


def _default_id(name: str, value: Any, index: int) -> str:
    if isinstance(value, (str, int, float, bool)) or value is None:
        return str(value)
    name_attr = getattr(value, "name", None)
    if isinstance(name_attr, str):
        return name_attr
    return f"{name}{index}"


def _expand_parametrized_tests(cls: type) -> None:
    for name, fn in list(vars(cls).items()):
        stacked = getattr(fn, _PARAMETRIZE_ATTR, None)
        if stacked is None:
            continue
        delattr(cls, name)
        for combination in itertools.product(*stacked):
            case_id = "-".join(case_id for case_id, _ in combination)
            kwargs = {}
            for _, values in combination:
                kwargs.update(values)
            case_name = _unique_test_name(cls, f"{name}_{_safe_name(case_id)}")
            setattr(cls, case_name, _bind_test_case(fn, case_name, kwargs))


def _safe_name(case_id: str) -> str:
    return re.sub(r"_+", "_", re.sub(r"\W", "_", case_id)).strip("_") or "case"


def _unique_test_name(cls: type, name: str) -> str:
    candidate, n = name, 1
    while candidate in vars(cls):
        n += 1
        candidate = f"{name}_{n}"
    return candidate


def _bind_test_case(
    fn: Callable[..., Any], name: str, kwargs: Dict[str, Any]
) -> Callable[..., Any]:
    if inspect.iscoroutinefunction(fn):

        @functools.wraps(fn)
        async def case(self: Any) -> None:
            await fn(self, **kwargs)

    else:

        @functools.wraps(fn)
        def case(self: Any) -> None:
            result = fn(self, **kwargs)
            if inspect.isawaitable(result):
                # A sync wrapper around an async test would otherwise pass without running it.
                raise TypeError(
                    f"{name} returned {result!r} instead of running: make it async"
                )

    return _as_test_case(case, fn, name)


def _as_test_case(
    case: Callable[..., Any], fn: Callable[..., Any], name: str
) -> Callable[..., Any]:
    case.__name__ = name
    case.__qualname__ = f"{fn.__qualname__.rsplit('.', 1)[0]}.{name}"
    case.__dict__.pop(_PARAMETRIZE_ATTR, None)
    return case


def buck_test(
    inplace: bool | None = None,
    data_dir: Optional[str] = "",
    # Accepted values are specified in SKIPPABLE_PLATFORMS
    skip_for_os: List[str] = [],  # noqa: B006 value is read-only
    # Materializer implementations (`BUCK2_TEST_MATERIALIZER` values) the test does not apply
    # to, for tests that assert on one implementation's own state or output. See
    # `buck2_core_tests`.
    skip_for_materializer: List[str] = [],  # noqa: B006 value is read-only
    allow_soft_errors: bool = False,
    extra_buck_config: Optional[Dict[str, Dict[str, str]]] = None,
    skip_final_kill: bool = False,
    setup_eden: bool = False,
    disable_daemon_cgroup: bool = True,
    write_invocation_record: bool = False,
    allow_daemon_panics: bool = False,
) -> Callable[..., Any]:
    """
    Defines a buck test. This is a must have decorator on all test case functions.

    Parameters:
        inplace:
            A bool for whether to run tests in-repo.
            If false, runs test under a sandbox repo. If `data_dir` or `data` are set on the target,
            the sandbox repo will be initialized with the contents of that directory. This can be
            disabled by setting `data_dir = None` on the test, or the test can set
            `data_dir = "subdir"` to just use the contents of a subdirectory.
            If true, runs test in fbsource.
        data_dir:
            data_dir is an optional string.
            If data_dir is set, then data_dir is the directory that contains test project data to
            copy, or the working directory relative to the cwd.
        skip_for_os:
            List of OS to skip the test on.
        skip_for_materializer:
            Materializer implementations to skip the test on, when the test target runs as a
            variant against another implementation than the default.
        allow_soft_errors:
            Like it says in the arg name. The default is to hard error.
        extra_buck_config:
            A optional dict of extra buck config to add to the test.
            The key is the section name, the value is a dict of key value pairs.
        skip_final_kill:
            Don't run a `buck2 kill` or `buck2 clean` at the end of the test
        setup_eden:
            Whether or not to set up an EdenFS repo for this test. Only matters for inplace=False.
            Note that this will slow the test down, so it should not be widely enabled.
        allow_daemon_panics:
            By default an isolated test fails if its daemon panicked at any point, even after
            the command finished. Set this if the test deliberately makes the daemon panic.
            That includes tests that expect a soft error from the daemon, since
            BUCK2_HARD_ERROR turns those into panics.
    """

    if inplace and data_dir == "":
        data_dir = None

    if _is_core_test():
        if inplace is not None:
            raise Exception(
                "Don't set `inplace` in `tests/core` - these tests are always isolated"
            )

        inplace = False
    else:
        if inplace is None:
            raise Exception("`inplace` must be set for `buck_test()`")

    # Set up arguments to use for the buck fixture.

    # The test passes without running rather than skipping: a skip is a failure to tpx.
    for p in skip_for_os:
        if p not in SKIPPABLE_PLATFORMS:
            raise Exception(f"skip_for_os must specifiy one of {SKIPPABLE_PLATFORMS}")
    # Without a variant's environment a test runs against the daemon's default implementation.
    skipped = (
        platform.system().lower() in skip_for_os
        or os.environ.get("BUCK2_TEST_MATERIALIZER", "deferred")
        in skip_for_materializer
    )

    if data_dir is not None and inplace:
        raise Exception(
            "`data_dir` is not an allowed parameter for an `inplace=True`test"
        )

    marker = BuckTestMarker(
        inplace=inplace,
        data_dir=data_dir,
        allow_soft_errors=allow_soft_errors,
        extra_buck_config=extra_buck_config or {},
        skip_final_kill=skip_final_kill,
        setup_eden=setup_eden,
        disable_daemon_cgroup=disable_daemon_cgroup,
        write_invocation_record=write_invocation_record,
        allow_daemon_panics=allow_daemon_panics,
    )

    def decorate(fn: Callable[..., Any]) -> Callable[..., Any]:
        if not _is_test_method(fn):
            raise TypeError(
                f"`buck_test` decorates the methods of a `BuckTestCase`, and {fn.__qualname__} "
                "takes no `self`"
            )
        test = _passing_test_method(fn) if skipped else _buck_test_method(marker, fn)
        test.buck_test_marker = marker  # pyrefly: ignore [missing-attribute]
        return test

    return decorate


def env(key: str, value: str) -> Callable[..., Any]:
    """
    Decorator for adding an environment variable to a test case.
    For example, @env("BUCK_LOG", "info")
    """

    def inner_decorator(fn: BuckTestFn) -> Callable[..., Any]:
        async def wrapped(fn: BuckTestFn, *args: Any, **kwargs: Any) -> None:
            _buck_argument(args, kwargs).set_env(key, value)
            return await fn(*args, **kwargs)

        return decorator(wrapped, fn)

    return inner_decorator


def windows_cmd_option(key: WindowsCmdOption, value: bool) -> Callable[..., Any]:
    """
    Decorator for specifying the state for cmd.exe's specified key feature
    For example, @windows_cmd_option(WindowsCmdOption.DelayedExpansion, True)
    """

    def inner_decorator(fn: BuckTestFn) -> Callable[..., Any]:
        async def wrapped(fn: BuckTestFn, *args: Any, **kwargs: Any) -> None:
            _buck_argument(args, kwargs).set_windows_cmd_option(key, value)
            return await fn(*args, **kwargs)

        return decorator(wrapped, fn)

    return inner_decorator


def is_deployed_buck2() -> bool:
    """
    This function detects whether or not you are using a deployed version of buck2
    so you can skip certain rule tests to only use deployed buck2.
    This may break deployed buck2, so please make sure this only affects tests for rules
    that buck2 users are not using.

    Example of a test case that passes without running with deployed buck2:
        if is_deployed_buck2():
            return
    """
    return os.environ.get("TEST_EXECUTABLE") == "buck2"


def get_mode_from_platform(
    mode: str = "dev",
    prefix: bool = True,
    skip_validation_i_know_what_im_doing: bool = False,
) -> str:
    if not skip_validation_i_know_what_im_doing and (mode not in ("dev", "opt")):
        raise Exception(f"Invalid mode: {mode}")

    def modefile_basename() -> str:
        if sys.platform == "darwin":
            if mode.startswith("dev"):
                return "mac"
            else:
                return "opt-mac"
        elif sys.platform == "win32":
            if mode.startswith("dev"):
                return "win"
            else:
                return "opt-win"
        if mode.startswith("dev"):
            return "dev"
        else:
            return "opt"

    if prefix:
        return f"@fbcode//mode/{modefile_basename()}"

    return modefile_basename()
