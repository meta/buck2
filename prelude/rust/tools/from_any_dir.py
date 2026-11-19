#!/usr/bin/env python3
# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

"""
See explanation in buck2/prelude/rust/cargo_buildscript.bzl:_make_cc_shim
"""

import argparse
import os
import sys
from pathlib import Path
from pprint import pformat
from typing import NamedTuple


class Args(NamedTuple):
    cwd: Path
    cc: list[str]


def arg_parse() -> Args:
    parser = argparse.ArgumentParser()
    parser.add_argument("--cwd", type=Path, required=True)
    parser.add_argument("cc", nargs=argparse.REMAINDER, type=str)
    return Args(**vars(parser.parse_args()))


def main():
    args = arg_parse()

    original_cwd = args.cwd.resolve()
    interim_cwd = Path.cwd().resolve()
    if original_cwd.is_relative_to(interim_cwd):
        interim_cwd = interim_cwd.relative_to(original_cwd, walk_up=True)

    placeholder = "\\${..}\\" if os.name == "nt" else "${..}/"
    cc = [arg.replace(placeholder, f"{interim_cwd}{os.sep}") for arg in args.cc]

    # TODO: figure out where this comes from and why it doesn't work
    #   Action failed: fbsource//third-party/rust/vendor/aws-lc-sys:0.37-build-script-main-run (buildscript)
    #   COMPILER: buck-out/v2/art/fbsource/220d36cb84017ab6/third-party/rust/vendor/aws-lc-sys/__0.37-build-script-main-run__/__cc_shim.sh
    #   ARGS: ["-Wall", "-Wextra", "-gdwarf-5", "-fembed-bitcode=off", "buck-out/v2/art/fbsource/220d36cb84017ab6/third-party/rust/vendor/aws-lc-sys/__0.37-build-script-main-run__/cwd/aws-lc/tests/compiler_features_tests/memcmp_invalid_stripped_check.c", "-Wno-unused-parameter", "-o", "buck-out/v2/art/fbsource/220d36cb84017ab6/third-party/rust/vendor/aws-lc-sys/__0.37-build-script-main-run__/OUT_DIR/memcmp_invalid_stripped_check"]
    #   ERROR: ld.lld: error: unable to find library -lomp
    #   OUTPUT: Failed to compile memcmp_invalid_stripped_check
    cc = list(filter(lambda arg: arg != "-Wl,-lomp", cc))

    # TODO: figure out why this flag passes `is_flag_supported` and is then rejected by the assembler.
    #
    #   Action failed: fbsource//third-party/rust/vendor/aws-lc-sys:0.42-build (buildscript)
    #   clang: error: unsupported argument '--debug-prefix-map=/re_cwd/buck-out/v2/art/fbsource/third-party/rust/vendor/aws-lc-sys/__0.42-build__/output_artifacts/cwd=' to option '-Wa,'
    #
    # The relevant code in third-party/rust/vendor/aws-lc-sys-0.42.0/builder/cc_builder.rs is:
    #
    #   let asm_flag = format!("-Wa,--debug-prefix-map={path_str}=");
    #   if cc_build.is_flag_supported(&asm_flag).unwrap_or(false) {
    #       cc_build.asm_flag(asm_flag);
    #   }
    cc = list(filter(lambda arg: not arg.startswith("-Wa,--debug-prefix-map="), cc))

    os.chdir(args.cwd)
    try:
        os.execl(cc[0], cc[0], *cc[1:])
    except Exception:
        print(f"exec failed: {pformat(cc)}", file=sys.stderr)
        raise


if __name__ == "__main__":
    main()
