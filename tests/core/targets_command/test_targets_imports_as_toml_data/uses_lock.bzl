# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

# Load a file whose extension (.lock) does not imply TOML, using the
# `?as=toml` format hint to parse it as TOML anyway.
load(":uv.lock?as=toml", _data = "value")

data = _data

def test():
    pass
