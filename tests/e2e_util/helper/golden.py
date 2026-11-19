# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.


import os
import re
import typing
from pathlib import Path

GOLDEN_DIRECTORY = "fixtures/"

# A CI label as it appears in JSON output, e.g. `"ci:overwrite",`.
_CI_LABEL_LINE = re.compile(r'^\s*"ci:[^"]*",?$')


def _prepend_header(content: str) -> str:
    return (
        f"# This file is {'@'}generated, "
        f"regenerate by re-running test with `-- --env BUCK2_UPDATE_GOLDEN=1` appended to the test command\n\n{content}"
    )


def _remove_ci_labels(content: str) -> str:
    # this label is only added for CI jobs, causing inconsistenty between local test and ci test.
    # Examples:
    #  "ci:overwrite",
    #  "ci:diff:linux:@fbcode//mode/dev-lg",
    #  "ci:continuous:linux:@fbcode//mode/dev-lg",
    new_content = []
    for line in content.splitlines():
        if _CI_LABEL_LINE.match(line):
            continue
        new_content.append(line)
    return "\n".join(new_content)


def _normalize_newlines(content: str) -> str:
    """
    We use golden() with text data so in the interest of being a bit more
    platform independent we just normalize the newlines.
    """
    return "".join([line + "\n" for line in content.splitlines()])


def _test_repo_data_src() -> str:
    # `TEST_REPO_DATA_SRC` is set in the test runner
    dir = os.getenv("TEST_REPO_DATA_SRC")
    assert dir, "TEST_REPO_DATA_SRC must be set"
    return dir


def _unified_diff(
    *,
    left: str,
    right: str,
    file: str,
    context: int,
) -> str:
    import difflib

    return "".join(
        difflib.unified_diff(
            left.splitlines(keepends=True),
            right.splitlines(keepends=True),
            fromfile=file,
            tofile=file,
            n=context,
        ),
    )


def _is_update_invocation() -> bool:
    return os.getenv("BUCK2_UPDATE_GOLDEN") is not None


# Output is a map of `rel_path`-relative files to their expected values
def golden_dir(*, output: typing.Dict[str, str], rel_path: str) -> None:
    assert "golden" in rel_path, f"Golden path `{rel_path}` must contain `golden`"

    rel_path_path = Path(rel_path)

    for file, contents in output.items():
        golden(
            output=contents,
            rel_path=str(rel_path_path.joinpath(Path(file))),
        )

    # Check that there are no extra files
    path_in_src = Path(_test_repo_data_src()).joinpath(rel_path_path)

    for file in path_in_src.glob("**/*"):
        if file.is_dir():
            continue
        rel_file_path = str(file.relative_to(path_in_src))
        if rel_file_path not in output:
            if _is_update_invocation():
                file.unlink()
            else:
                raise AssertionError(
                    f"Extra golden file `{rel_file_path}` found, please remove it"
                )


def golden(*, output: str, rel_path: str) -> None:
    assert "golden" in rel_path, f"Golden path `{rel_path}` must contain `golden`"

    output = _prepend_header(output)
    output = _normalize_newlines(output)

    path_in_src = os.path.join(_test_repo_data_src(), rel_path)

    if _is_update_invocation():
        Path(path_in_src).parent.mkdir(parents=True, exist_ok=True)
        with open(path_in_src, "w", encoding="utf-8") as f:
            f.write(output)
        return

    assert os.path.exists(path_in_src), f"Golden path `{path_in_src}` must exist"

    with open(path_in_src, "r", encoding="utf-8") as f:
        expected = f.read()

    if _remove_ci_labels(expected) != _remove_ci_labels(output):
        unified_diff = _unified_diff(
            left=expected,
            right=output,
            file=path_in_src,
            context=3,
        )
        raise AssertionError(
            f"Expected golden file to match actual\n"
            f"\n\n{unified_diff}\n\n"
            "Re-run test with `-- --env BUCK2_UPDATE_GOLDEN=1` appended to the test command to regenerate the files"
        )


# Replace 128-bit configuration with placeholder.
def _replace_cfg_hash(s: str) -> str:
    return re.sub(r"\b[0-9a-f]{16}\b", "<HASH>", s)


def golden_replace_cfg_hash(*, output: str, rel_path: str) -> None:
    golden(
        output=_replace_cfg_hash(output),
        rel_path=rel_path,
    )


def golden_replace_temp_path(*, output: str, rel_path: str, tmp_path: str) -> None:
    # Escaping backslashes are needed for windows paths
    tmp_path_escaped = tmp_path.replace("\\", "\\\\")
    golden(
        output=output.replace(tmp_path_escaped, "tmp-path").replace("\\\\", "/"),
        rel_path=rel_path,
    )


def sanitize_hashes(s: str) -> str:
    # Remote message hashes
    s = re.sub(r"\b[0-9]{16,}\b", "<STRING_HASH>", s)
    # Remove configuration hashes
    # This is so bad... we don't force these hashes to print as 16
    # characters... and that's hard to fix because we don't allow changes to
    # change action digests.
    s = re.sub(r"\b[0-9a-f]{12,16}\b", "<HASH>", s)
    # And action digests
    return re.sub(r"\b[0-9a-f]{40}:[0-9]{1,3}\b", "<DIGEST>", s)


# C++ libraries linked into buck2 (and the tools it spawns) can emit glog-format
# lines such as `I0623 15:40:41.926481 128942 Hash.cpp:327] tiHash seed: ...ull`
# to stderr during process init, with any of glog's or folly logging's severity
# prefixes (`V`, `I`, `W`, `E`, `C`, `F`). Their timestamps, PIDs, and source locations
# vary between runs, so they leak non-deterministically into captured stderr and
# must be dropped before comparing against goldens. The marker may be indented
# (e.g. when nested inside an embedded daemon stderr block), so we match it
# anywhere in the line.
_GLOG_LINE_RE = re.compile(r".*[VIWECF]\d{4} \d{2}:\d{2}:\d{2}\.\d{6}.*\n?")
# The buck2 client's own `tracing` output goes to stderr too, as lines such as
# `2026-10-15T02:15:11.181159Z  WARN buck2_client_ctx::events_ctx: Finalizing
# 'event log' took 1.38s`, possibly wrapped in ANSI color escapes. Which of
# those appear depends on timing (that one shows up under load), so they are
# dropped the same way.
_TRACING_LINE_RE = re.compile(
    r"^(?:\x1b\[[0-9;]*m)*\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(?:\.\d+)?Z(?:\x1b\[[0-9;]*m)*"
    r"\s+(?:\x1b\[[0-9;]*m)*\s*(?:TRACE|DEBUG|INFO|WARN|ERROR)(?:\x1b\[[0-9;]*m)*\b.*\n?",
    flags=re.MULTILINE,
)
# Some fbcode binaries have this removed option baked into their allocator
# configuration, so the warning cannot be suppressed through MALLOC_CONF.
_JEMALLOC_INVALID_CONF_RE = re.compile(
    r"^[ \t]*<jemalloc>: Invalid conf pair: experimental_infallible_new:true\n?",
    flags=re.MULTILINE,
)


def strip_glog_lines(s: str) -> str:
    # Drops whole matching lines (leading indentation and trailing newline
    # included) while leaving every other newline intact, so callers' trailing
    # and blank-line structure is preserved.
    return _TRACING_LINE_RE.sub("", _GLOG_LINE_RE.sub("", s))


def strip_jemalloc_invalid_conf(s: str) -> str:
    return _JEMALLOC_INVALID_CONF_RE.sub("", s)


# A submitted remote action reports whether the remote execution service ran it, served it
# from its own cache, or merged it with an identical execution already in flight. The last two
# depend on what other builds are doing with the same action digest at the same time.
_RE_SUBMITTED_ORIGIN_RE = re.compile(
    r"Remote action (?:served from the remote execution cache|deduplicated by remote execution),"
)


def sanitize_remote_action_origin(s: str) -> str:
    return _RE_SUBMITTED_ORIGIN_RE.sub("Remote action,", s)


def sanitize_stderr(s: str) -> str:
    s = sanitize_remote_action_origin(strip_jemalloc_invalid_conf(strip_glog_lines(s)))
    # Remove all timestamps
    s = re.sub(r"\[.{29}\]", "[<TIMESTAMP>]", s)
    # Remove all UUIDs
    s = re.sub(
        r"\b[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}\b", "<UUID>", s
    )
    # Sanitize daemon process info file paths.
    s = re.sub(
        r"^    Daemon process info from .+:?$",
        "    Daemon process info from <BUCKD_INFO>",
        s,
        flags=re.MULTILINE,
    )
    s = re.sub(
        r"^    daemon dir: .+$", "    daemon dir: <DAEMON_DIR>", s, flags=re.MULTILINE
    )
    s = re.sub(r"^    pid: \d+$", "    pid: <PID>", s, flags=re.MULTILINE)
    s = re.sub(
        r"^    endpoint: tcp:\d+$", "    endpoint: tcp:<PORT>", s, flags=re.MULTILINE
    )
    s = re.sub(r"^    version: \S+$", "    version: <VERSION>", s, flags=re.MULTILINE)
    # Remove "Commands" line
    s = re.sub(r"Commands: .+", "Commands: <COMMAND_STATS>", s)
    # Remove "Cache hits" percentage
    s = re.sub(r"Cache hits: .+", "Cache hits: <CACHE_STATS>", s)
    # Remove "Network" line
    s = re.sub(r"Network: .+", "Network: <NETWORK_STATS>", s)
    # Remove thread ID & path from "panicked at" line
    s = re.sub(r"\([0-9]+\) panicked at .+", "(<THREAD_ID>) panicked at <PATH>", s)
    return sanitize_hashes(s)


def sanitize_daemon_stderr(s: str) -> str:
    """Sanitize daemon tracing output embedded in error messages.

    Daemon stderr uses tracing-subscriber format with ANSI codes and
    timestamps like '2026-03-11T21:36:35.686733Z'.
    """
    # Strip ANSI escape codes
    s = re.sub(r"\x1b\[[0-9;]*m", "", s)
    # Sanitize daemon tracing timestamps (ISO 8601 with fractional seconds)
    s = re.sub(r"\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}\.\d+Z", "<DAEMON_TIMESTAMP>", s)
    # Sanitize PIDs
    s = re.sub(r"PID: \d+", "PID: <PID>", s)
    # Sanitize endpoint ports
    s = re.sub(r"Endpoint: tcp:\d+", "Endpoint: tcp:<PORT>", s)
    # Sanitize version strings
    s = re.sub(r"Version: \S+", "Version: <VERSION>", s)
    # Sanitize scratch paths
    s = re.sub(r"/data/users/\S+", "<SCRATCH_PATH>", s)
    # Sanitize timed out duration
    s = re.sub(r"timed out after [\d.]+s", "timed out after <DURATION>", s)
    # Strip lines with env override logging (these vary by environment)
    s = re.sub(r"^.*Env override found.*\n", "", s, flags=re.MULTILINE)
    # BPFJailer is only used on hosts where the jail is available.
    s = re.sub(r"^.*Exited BPFJailer jail.*\n", "", s, flags=re.MULTILINE)
    # Strip trailing whitespace on each line
    s = re.sub(r" +$", "", s, flags=re.MULTILINE)
    return sanitize_stderr(s)


def sanitize_stacktrace(s: str) -> str:
    s = sanitize_stderr(s)
    lines = s.splitlines()

    # Rust wraps a frame's demangled symbol across multiple physical lines when
    # it is long (e.g. deeply generic async future types). Only the first such
    # line carries the ` NN:` prefix; the wrapped continuations do not, so the
    # per-line filter below cannot catch them and they leak non-deterministically
    # into the golden depending on which frames are live at crash time. Drop the
    # whole frame block between the "stack backtrace:" header and the terminating
    # "note:" line (both kept) so no frame content leaks regardless of wrapping.
    start = next(
        (
            i
            for i, x in enumerate(lines)
            if re.match(r"\[<TIMESTAMP>\].*stack backtrace:\s*$", x)
        ),
        None,
    )
    if start is not None:
        end = next(
            (
                i
                for i in range(start + 1, len(lines))
                if re.match(r"\[<TIMESTAMP>\]\s+note:", lines[i])
            ),
            None,
        )
        if end is not None:
            lines = lines[: start + 1] + lines[end:]

    return "\n".join(
        filter(
            lambda x: re.match(r"\[<TIMESTAMP>\]((\s+\d+:)|(\s+at )).*", x) is None,
            lines,
        )
    )


# Build report errors can change based on minor test changes such as
# 1. Adding a target in TARGETS.fixture
# 2. Line number changing due to code moving around
# Sanitize so that we only check the important bits of the error message
def sanitize_build_report_error(s: str) -> str:
    # Simplify analysis error message (Can change due to line number changes)
    s = re.sub(
        r"Error running analysis for.*\"", 'Error running analysis for <IRRELEVANT>"', s
    )
    # Simplify the Unknown target error (Can change due to number of targets in TARGETS.fixture)
    s = re.sub(
        r"Unknown target `.*` from package .*\"",
        'Unknown target `<TARGET>` from package <IRRELEVANT>"',
        s,
    )

    return sanitize_hashes(s)


def sanitize_build_report(report: dict) -> None:
    def sanitize_value(value: object) -> object:
        if isinstance(value, str):
            return sanitize_remote_action_origin(strip_jemalloc_invalid_conf(value))
        if isinstance(value, list):
            return [sanitize_value(item) for item in value]
        if isinstance(value, dict):
            return {key: sanitize_value(item) for key, item in value.items()}
        return value

    for key, value in report.items():
        report[key] = sanitize_value(value)

    del report["trace_id"]
    del report["project_root"]

    # String cache keys can vary due to differences in platform hashes within the message
    if "strings" in report:
        # Sort by sanitized values
        strings = dict(
            sorted(
                report["strings"].items(),
                key=lambda item: sanitize_hashes(item[1]),
            )
        )
        # Create a new dict where the keys are 1 + a large number
        # in order for it to still be in the format of a string hash
        updated_strings = {}
        start = 10000000000000000
        for i, v in enumerate(strings.values()):
            updated_strings[i + start] = v

        report["strings"] = updated_strings


def sanitize_python(s: str, project_dir: Path) -> str:
    # Strip absolute project dir prefix
    s = s.replace(f"{project_dir}/", "")
    # Match python38 error formatting (can be removed when python38 is removed everywhere)
    s = re.sub(r" *\^+", "", s)
    s = s.replace("SyntaxError: invalid syntax", "IndentationError: unexpected indent")
    s = s.replace("[syntax] Syntax error!", "[indentation] Indentation error!")
    s = re.sub(r"(\\n)+", r"\\n", s)
    return s


def strip_waiting_on(s: str) -> str:
    # Strip "Waiting on" lines
    return "\n".join(filter(lambda x: "Waiting on" not in x, s.splitlines()))
