# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

load("@fbcode_macros//build_defs:native_rules.bzl", "alias")

def alias_pem(pems: list[str]):
    for pem in pems:
        alias(
            name = pem,
            actual = "fbsource//xplat/folly/io/async/test/certs:{pem}".format(pem = pem),
        )

def alias_pem_for_xplat(pems: list[str]):
    # in xplat these pem files are exported in //xplat/folly/io/async/test/certs
    for pem in pems:
        alias(
            name = pem,
            actual = "//xplat/folly/io/async/test/certs:{pem}".format(pem = pem),
        )
