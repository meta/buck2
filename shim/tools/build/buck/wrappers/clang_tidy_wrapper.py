# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

"""
Stub clang-tidy wrapper for the OSS buck2 build.

`prelude/cxx/tools/defs.bzl::cxx_internal_tools` declares a default
`clang_tidy_wrapper = "fbcode//tools/build/buck/wrappers:clang_tidy_wrapper"`,
introduced by D102047xxx (Krueger, 2026-04-25). The OSS shim aliases
`fbcode` to `gh_facebook_buck2_shims_meta`, so the prelude looks for that
target inside the shim. This file exists purely to provide a target with
RunInfo at the expected path; clang-tidy itself isn't wired up for the
OSS bootstrap.

If a build action ever actually invokes this wrapper, exit non-zero with a
clear message so it surfaces immediately rather than silently producing
empty diagnostics.
"""

import sys

if __name__ == "__main__":
    print(
        "clang-tidy is not configured for the OSS buck2 build. "
        "If this wrapper was invoked, the cxx toolchain has been asked to "
        "produce clang-tidy diagnostics — disable that path or wire up a "
        "real clang_tidy_wrapper.",
        file=sys.stderr,
    )
    sys.exit(1)
