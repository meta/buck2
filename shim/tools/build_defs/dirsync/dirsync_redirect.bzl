# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

def _read():
    return None

def _soname(name):
    return name

def _create_alias(*_args, **_kwargs):
    pass

def _write(*_args, **_kwargs):
    pass

dirsync_redirect = struct(
    create_alias = _create_alias,
    read = _read,
    soname = _soname,
    write = _write,
)
