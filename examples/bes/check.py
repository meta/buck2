# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

"""Run buck2 against a recording Build Event Service and check what it received.

    python3 check.py --buck2 <path to buck2>

The Python running this needs `grpcio` and `grpcio-tools`. It generates the
stubs from the protos under `remote_execution/oss`, starts
`fake_bes_server.py`, points this project's `[bes]` section at it, and checks
four commands:

- a build that succeeds streams `started` first and `build_finished` with
  SUCCESS, then closes the stream;
- a build that fails reports a non-zero exit code in `build_finished`;
- a test run reports each test's status;
- `buck2 kill --bes-drain-timeout` waits until a slow server has read and
  acknowledged a stream the daemon still held when its command ended.
"""

import argparse
import json
import os
import re
import subprocess
import sys
import tempfile
import time
from pathlib import Path

HERE = Path(__file__).resolve().parent
PROTOS = HERE.parent.parent / "remote_execution" / "oss"


def generate_stubs(out: Path) -> None:
    bes = PROTOS / "bes_grpc_proto" / "proto"
    google = PROTOS / "google_grpc_proto" / "proto"
    bep = PROTOS / "bazel_bep_proto"
    sources = [
        *bes.glob("*.proto"),
        *(google / "google" / "api").glob("*.proto"),
        google / "google" / "rpc" / "status.proto",
        *(bep / "proto").glob("*.proto"),
    ]
    subprocess.run(
        [
            sys.executable,
            "-m",
            "grpc_tools.protoc",
            f"-I{bes}",
            f"-I{google}",
            f"-I{bep}",
            f"--python_out={out}",
            f"--grpc_python_out={out}",
            *map(str, sources),
        ],
        check=True,
    )


class Server:
    def __init__(self, work: Path, name: str, *flags: str) -> None:
        self.record = work / f"{name}.jsonl"
        port_file = work / f"{name}.port"
        self.log = open(work / f"{name}.log", "w")
        self.process = subprocess.Popen(
            [
                sys.executable,
                str(HERE / "fake_bes_server.py"),
                f"--stubs={work / 'stubs'}",
                f"--record={self.record}",
                f"--port-file={port_file}",
                *flags,
            ],
            stdout=self.log,
            stderr=subprocess.STDOUT,
        )
        deadline = time.monotonic() + 30
        while not port_file.exists():
            if self.process.poll() is not None or time.monotonic() > deadline:
                raise RuntimeError(
                    f"fake BES server did not start, see {self.log.name}"
                )
            time.sleep(0.1)
        self.port = int(port_file.read_text())

    def stop(self) -> None:
        self.process.terminate()
        self.process.wait(timeout=30)
        self.log.close()

    def stream(self, build_id: str) -> list:
        rows = [json.loads(line) for line in self.record.read_text().splitlines()]
        return [row for row in rows if row.get("invocation_id") == build_id]


class Check:
    def __init__(self, buck2: str, work: Path) -> None:
        self.buck2 = buck2
        self.work = work
        self.failures = []
        self.commands = 0

    def configure(self, server: Server) -> None:
        (HERE / ".buckconfig.local").write_text(
            "[bes]\n"
            f"  backend = grpc://127.0.0.1:{server.port}\n"
            "  event_format = bazel\n"
        )

    def run(self, *args: str) -> tuple:
        self.commands += 1
        build_id_file = self.work / f"build-id-{self.commands}"
        result = subprocess.run(
            [self.buck2, *args, f"--write-build-id={build_id_file}"],
            cwd=HERE,
            capture_output=True,
            text=True,
        )
        build_id = build_id_file.read_text().strip()
        print(
            f"$ buck2 {' '.join(args)}: exit {result.returncode}, build ID {build_id}"
        )
        return result.returncode, build_id

    def kill(self, *args: str) -> str:
        result = subprocess.run(
            [self.buck2, "kill", *args],
            cwd=HERE,
            capture_output=True,
            text=True,
        )
        output = result.stdout + result.stderr
        print(f"$ buck2 kill {' '.join(args)}: exit {result.returncode}")
        for line in output.splitlines():
            if "BES:" in line:
                print(f"  {line.strip()}")
        return output

    def expect(self, ok: bool, what: str) -> None:
        print(f"  {'ok' if ok else 'FAILED'}: {what}")
        if not ok:
            self.failures.append(what)

    def expect_complete_stream(self, stream: list, exit_ok: bool) -> None:
        events = [row for row in stream if row["rpc"] == "stream"]
        self.expect(
            bool(events), f"the server received the stream ({len(events)} events)"
        )
        if not events:
            return
        seqs = [row["seq"] for row in events]
        self.expect(
            seqs == list(range(1, len(seqs) + 1)),
            "sequence numbers run from 1 without a gap",
        )
        self.expect(
            events[0].get("bazel_id") == "started", "the first event is `started`"
        )
        finished = [row for row in events if row.get("bazel_id") == "build_finished"]
        self.expect(len(finished) == 1, "one `build_finished` event")
        if finished:
            code = finished[0].get("exit_code", 0)
            self.expect(
                (code == 0) == exit_ok,
                f"`build_finished` reports exit code {code} "
                f"({finished[0].get('exit_name')})",
            )
        self.expect(
            any(row.get("last_message") for row in events),
            "one event is marked `last_message`",
        )
        self.expect(
            events[-1]["kind"] == "component_stream_finished",
            "the stream ends with `component_stream_finished`",
        )
        self.expect(
            any(row["rpc"] == "stream_end" for row in stream),
            "the client closed the stream",
        )


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--buck2", required=True)
    args = parser.parse_args()

    work = Path(tempfile.mkdtemp(prefix="buck2-bes-check-"))
    (work / "stubs").mkdir()
    generate_stubs(work / "stubs")
    check = Check(os.path.abspath(args.buck2), work)
    check.kill()

    server = Server(work, "acks")
    check.configure(server)
    try:
        rc, build_id = check.run("build", "//:ok")
        check.expect(rc == 0, "`buck2 build //:ok` succeeds")
        build_ok = build_id
        rc, build_id = check.run("build", "//:fail")
        check.expect(rc != 0, "`buck2 build //:fail` fails")
        build_fail = build_id
        rc, build_id = check.run("test", "//:test_pass", "//:test_fail")
        check.expect(rc != 0, "`buck2 test` fails, as `//:test_fail` does")
        test = build_id
        check.kill("--bes-drain-timeout=60s")
    finally:
        server.stop()

    print("A build that succeeds:")
    check.expect_complete_stream(server.stream(build_ok), exit_ok=True)
    print("A build that fails:")
    check.expect_complete_stream(server.stream(build_fail), exit_ok=False)
    print("A test run:")
    stream = server.stream(test)
    check.expect_complete_stream(stream, exit_ok=False)
    statuses = {
        row["label"]: row["test_status"]
        for row in stream
        if row.get("bazel_id") == "test_result"
    }
    check.expect(
        statuses.get("//:test_pass") == "PASSED",
        f"`//:test_pass` is PASSED (results: {statuses})",
    )
    check.expect(
        statuses.get("//:test_fail") == "FAILED",
        "`//:test_fail` is FAILED",
    )

    # This server reads each event late and acknowledges only at the end of a
    # stream, so the build returns while the daemon still holds most of its
    # events, and only a drain delivers them before the daemon exits.
    server = Server(work, "slow", "--ack-at-eof", "--read-delay=0.1")
    check.configure(server)
    try:
        rc, build_id = check.run("build", "//:ok")
        check.expect(rc == 0, "`buck2 build //:ok` succeeds")
        output = check.kill("--bes-drain-timeout=120s")
    finally:
        server.stop()

    print("`buck2 kill --bes-drain-timeout` after a build, with a slow server:")
    drained = re.search(
        r"BES: (\d+) of (\d+) streams acknowledged, (\d+) events pending", output
    )
    check.expect(drained is not None, "the kill reports the drain")
    if drained:
        acked, total, pending = map(int, drained.groups())
        check.expect(total >= 1, f"the daemon still held {total} stream(s) at the kill")
        check.expect(
            acked == total and pending == 0, "the server acknowledged them all"
        )
    check.expect_complete_stream(server.stream(build_id), exit_ok=True)

    (HERE / ".buckconfig.local").unlink()
    if check.failures:
        print(f"{len(check.failures)} check(s) failed; the recordings are in {work}")
        sys.exit(1)
    print("All checks passed.")


if __name__ == "__main__":
    main()
