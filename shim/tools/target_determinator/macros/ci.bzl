# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

# https://internalfb.com/code/fbsource/fbcode/target_determinator/macros/README.md

def _lbl(*_args):
    return ""

def _package(
    *_values,
    # starlark-lint-disable unused-argument
    overwrite = False,
):  # @unused
    pass

def _labels(*args):
    return []

ci = struct(
    package = _package,
    linux = _lbl,
    mac = _lbl,
    windows = _lbl,
    skip_test = _lbl,
    aarch64 = _lbl,
    x86_64 = _lbl,
    mode = _lbl,
    opt = _lbl,
    remove = _lbl,
    labels = _labels,
    modifiers = _lbl,
)
