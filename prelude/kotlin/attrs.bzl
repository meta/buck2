# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

def no_x_jdk_release_arg() -> dict[str, Attr]:
    return {
        "no_x_jdk_release": attrs.bool(
            default = False,
            doc = """
        By default, classic kotlin adds -Xjdk-release=java_version to the kotlinc arguments.
        Set this to True to not add -Xjdk-release=java_version to the kotlinc arguments.
        This is helpful if the -Xjdk-release= brings trouble since it is not 100% guaranteed to work as expected.
        See the [doc of kotlinc](https://kotlinlang.org/docs/compiler-reference.html#xjdk-release-version) for more detail.
""",
        ),
    }
