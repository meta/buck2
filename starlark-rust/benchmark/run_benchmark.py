#!/usr/bin/env python3

# Copyright 2018 The Starlark in Rust Authors.
# Copyright (c) Facebook, Inc. and its affiliates.
#
# Licensed under the Apache License, Version 2.0 (the "License");
# you may not use this file except in compliance with the License.
# You may obtain a copy of the License at
#
#     https://www.apache.org/licenses/LICENSE-2.0
#
# Unless required by applicable law or agreed to in writing, software
# distributed under the License is distributed on an "AS IS" BASIS,
# WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
# See the License for the specific language governing permissions and
# limitations under the License.


import argparse
import os
import subprocess
import tempfile
import time
from pathlib import Path


def compile_starlark():
    if "CARGO_TARGET_DIR" not in os.environ:
        raise Exception("Must set CARGO_TARGET_DIR, so we can find Cargo outputs")
    print("Building starlark")
    subprocess.run("cargo build --release --bin=starlark", shell=True, check=True)
    return os.environ["CARGO_TARGET_DIR"] + "/release/starlark"


def generate_benchmarks(dir):
    benchmark = Path(__file__).parent.joinpath("benchmark.py")

    with open(benchmark, "r") as file:
        src = file.read()

    # Find all the benchmarks
    benchmarks = [
        x[4:-3]
        for x in src.splitlines()
        if x.startswith("def benchmark") and x.endswith("():")
    ]

    outputs = {}
    for benchmark in benchmarks:
        # Whichever one is committed, make sure we switch it for this one
        src2 = src
        for x in benchmarks:
            src2 = src2.replace("print(" + x + "())", "print(" + benchmark + "())")
        output = Path(dir).joinpath(benchmark + ".py")
        with open(output, "w") as out:
            out.write(src2)
        outputs[benchmark] = output
    return outputs


def cmd(args):
    res = subprocess.run(args, capture_output=True)
    if res.returncode != 0:
        raise Exception(
            "Command failed: {}\nStdout: {}\nStderr: {}".format(
                args, res.stdout, res.stderr
            )
        )


def absh(a, b, repeat):
    a_time = 0
    b_time = 0
    runs = 0

    # Run a/b repeatedly, ignoring the first loop around
    for i in range(repeat + 1):
        start_time = time.time()
        cmd(a)
        middle_time = time.time()
        cmd(b)
        end_time = time.time()

        if i != 0:
            a_time += middle_time - start_time
            b_time += end_time - middle_time
            runs += 1
        print(".", end="", flush=True)

    print("")
    return (a_time / runs, b_time / runs)


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument(
        "--repeat",
        default=6,
        type=int,
        help="How many times to repeat",
    )
    parser.add_argument(
        "--starlark",
        default=None,
        type=str,
        help="Path to a prebuilt starlark binary; skips the cargo build",
    )
    parser.add_argument(
        "benchmarks",
        nargs="*",
        type=str,
        help="Benchmarks to run, if not specified, all benchmarks",
    )
    args = parser.parse_args()

    starlark = args.starlark or compile_starlark()
    with tempfile.TemporaryDirectory() as dir:
        benchmarks = generate_benchmarks(dir)
        for name, file in benchmarks.items():
            if len(args.benchmarks) == 0 or name in args.benchmarks:
                print("Benchmarking: " + name + " ", end="", flush=True)
                (py, st) = absh(("python3", file), (starlark, file), repeat=args.repeat)
                print(
                    "Python3 {:.2f}s, Starlark Rust {:.2f}s  Starlark/Python {:.2f}%".format(
                        py, st, (st / py) * 100
                    )
                )


if __name__ == "__main__":
    main()
