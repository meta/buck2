#!/usr/bin/env fbpython
# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

"""
Remaps swiftc arguments to be usable by swift-ide-test, and invokes
swift-ide-test with said arguments.
"""

import argparse
import dataclasses
import optparse
import subprocess as proc
import sys
from collections.abc import Iterable
from contextlib import contextmanager
from typing import Optional


@dataclasses.dataclass
class SwiftIDETestArguments:
    sdk: Optional[str]
    target: Optional[str]
    xcc: Iterable[str]
    defines: Iterable[str]
    frameworks: Iterable[str]
    includes: Iterable[str]
    resource_dir: str
    enable_cxx_interop: bool
    cxx_interoperability_mode: Optional[str]
    upcoming_features: Iterable[str]
    explicit_swift_module_map: Optional[str]
    swift_version: Optional[str]

    def to_args(self) -> list[str]:
        args = []
        if self.target:
            args.append("--target")
            args.append(self.target)

        if self.sdk:
            args.append("--sdk")
            args.append(self.sdk)

        for define in self.defines:
            args.append("-D")
            args.append(define)

        for include in self.includes:
            args.append("-I")
            args.append(include)

        for framework in self.frameworks:
            args.append("-F")
            args.append(framework)

        for xcc in self.xcc:
            args.append("--Xcc")
            args.append(xcc)

        args.append("--resource-dir")
        args.append(self.resource_dir)

        if self.enable_cxx_interop:
            args.append("-enable-experimental-cxx-interop")

        if self.cxx_interoperability_mode:
            # swift-ide-test only understands -enable-experimental-cxx-interop,
            # not the versioned -cxx-interoperability-mode=.
            args.append("-enable-experimental-cxx-interop")

        if self.upcoming_features:
            for feature in self.upcoming_features:
                args.append("-enable-upcoming-feature")
                args.append(feature)

        if self.explicit_swift_module_map:
            args.append("--explicit-swift-module-map-file")
            args.append(self.explicit_swift_module_map)

        if self.swift_version:
            args.append("-swift-version")
            args.append(self.swift_version)
        return args


class LongSingleDashOpt(optparse.Option):
    """
    This Option subclass allows for long arguments specified with single dashes,
    e.g. -sdk (the default implementation only allows long options with two
    dashes)
    """

    def _set_opt_strings(self, opts):
        for opt in opts:
            if len(opt) < 2:
                raise optparse.OptionError(
                    "invalid option string %r: "
                    "must be at least two characters long" % opt,
                    self,
                )
            elif len(opt) == 2:
                self._short_opts.append(opt)
            else:
                self._long_opts.append(opt)


class IgnoreUnknownLongSingleDashOptParser(optparse.OptionParser):
    """
    This OptionParser subclass allows for
    (a) long arguments specified with single dashes (e.g. -sdk)
    (b) ignoring unknown arguments
    The default OptionParser doesn't have either of these behaviors.
    """

    def __init__(self, *args, **kwargs):
        kwargs["option_class"] = LongSingleDashOpt
        # These are swiftc flags, not flags of this tool: with the optparse `-h`
        # registered, the joined-value rule below reads a value such as
        # `-headerpad_max_install_names` as `-h` and exits before `--out` is written.
        kwargs.setdefault("add_help_option", False)
        super().__init__(*args, **kwargs)

    def _process_args(self, largs, rargs, values):
        while rargs:
            try:
                arg = rargs[0]
                if arg == "--":
                    del rargs[0]
                    return
                elif arg[0:2] == "--":
                    self._process_long_opt(rargs, values)
                elif arg[:1] == "-" and len(arg) > 1:
                    # A registered long option such as `-Fsystem` wins over the short option
                    # `-F` with a joined value.
                    if len(arg) > 2 and (
                        arg.split("=", 1)[0] in self._long_opt
                        or arg[:2] not in self._short_opt
                    ):
                        self._process_long_opt(rargs, values)
                    else:
                        self._process_short_opts(rargs, values)
                elif self.allow_interspersed_args:
                    largs.append(arg)
                    del rargs[0]
                else:
                    return
            except optparse.BadOptionError:
                continue


def parse_swiftc_args(arguments: list[str]) -> SwiftIDETestArguments:  # noqa: C901
    """
    We can't use argparse to do our parsing because arguments like -Xcc
    need to accept arguments that are prefixed with `-`.

    optparse can handle this, and it's only soft deprecated (i.e. it should
    stay around, just not actively developed), so we should be safe to use it.

    Additionally, our subclasses above are safe, since optparse is no longer
    actively developed.
    """
    parser = IgnoreUnknownLongSingleDashOptParser()

    parser.add_option("-sdk", dest="sdk")
    parser.add_option("-target", dest="target")
    parser.add_option("-Xcc", action="append", default=[], dest="xcc")
    parser.add_option("-D", dest="defines", action="append", default=[])
    parser.add_option("-F", dest="frameworks", action="append", default=[])
    parser.add_option("-Fsystem", dest="frameworks", action="append")
    parser.add_option("-I", dest="includes", action="append", default=[])
    parser.add_option("-resource-dir", dest="resource_dir")
    parser.add_option(
        "-enable-experimental-cxx-interop",
        action="store_true",
        default=False,
        dest="enable_experimental_cxx_interop",
    )
    parser.add_option("-Xfrontend", action="append", default=[], dest="xfrontend")
    parser.add_option("-swift-version", dest="swift_version")
    parser.add_option("-cxx-interoperability-mode", dest="cxx_interoperability_mode")

    options, _ = parser.parse_args(arguments)

    frontend_parser = IgnoreUnknownLongSingleDashOptParser()
    frontend_parser.add_option(
        "-enable-upcoming-feature",
        dest="enable_upcoming_feature",
        action="append",
        default=[],
    )
    frontend_parser.add_option(
        "-explicit-swift-module-map-file", dest="explicit_swift_module_map"
    )
    frontend_options = frontend_parser.parse_args(options.xfrontend)[0]

    resource_dir = options.resource_dir
    if not resource_dir:
        raise SystemExit(
            "make_swift_interface: no -resource-dir in the swiftc flags; "
            "set resource_dir on the swift_toolchain"
        )

    return SwiftIDETestArguments(
        options.sdk,
        options.target,
        options.xcc,
        options.defines,
        options.frameworks,
        options.includes,
        resource_dir,
        options.enable_experimental_cxx_interop,
        options.cxx_interoperability_mode,
        frontend_options.enable_upcoming_feature,
        frontend_options.explicit_swift_module_map,
        options.swift_version,
    )


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description="Uses swift-ide-test to generate a swift interface",
        fromfile_prefix_chars="@",
    )
    parser.add_argument(
        "--swift-ide-test-tool",
        required=True,
        help="Path to swift-ide-test binary.",
    )
    parser.add_argument(
        "--module",
        required=True,
        help="Name of the module to generate the interface for.",
    )
    parser.add_argument(
        "--out",
        help="Path to output file.",
        default="-",
    )
    parser.add_argument(
        "arguments",
        nargs="*",
        default=[],
        help="File containing compiler arguments to use to invoke"
        + " swift-ide-test. Note these arguments should be in the format CC"
        + " expects, not swift-ide-test, as this tool converts the arguments"
        + " as needed",
    )
    return parser.parse_args()


@contextmanager
def open_or_stdout(out):
    if out == "-":
        yield sys.stdout
    else:
        with open(out, "w") as f:
            yield f


def main() -> None:
    args = parse_args()

    parsed = parse_swiftc_args(args.arguments)
    with open_or_stdout(args.out) as out:
        try:
            proc.run(
                [
                    args.swift_ide_test_tool,
                    "--source-filename=x",
                    "--print-module",
                    "--module-to-print",
                    args.module,
                    "--module-print-submodules",
                ]
                + parsed.to_args(),
                stdout=out,
                stderr=proc.PIPE,
                check=True,
                text=True,
            )
        except proc.CalledProcessError as exc:
            print("swift-ide-test error:", file=sys.stderr)
            sys.stderr.write(exc.stderr)
            sys.exit(exc.returncode)


if __name__ == "__main__":
    main()
