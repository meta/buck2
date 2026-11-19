# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

load("@prelude//rust:clippy_configuration.bzl", native_clippy_configuration = "clippy_configuration")

def clippy_configuration(name, clippy_toml_src, **kwargs):
    native_clippy_configuration(name = name, clippy_toml_src = clippy_toml_src, **kwargs)
