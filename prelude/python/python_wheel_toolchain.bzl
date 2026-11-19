# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

"""
Python wheel toolchain for configuring wheel building and repair tools.

This toolchain provides configuration for tools used in building and repairing
Python wheels, particularly for manylinux compliance via auditwheel.
"""

PythonWheelToolchainInfo = provider(
    doc = "Python wheel toolchain info",
    fields = {
        "abi": provider_field(str, default = "none"),
        # Default platform tag for wheels (e.g., linux_x86_64, manylinux_2_28_x86_64)
        "platform": provider_field(str, default = "any"),
        "python": provider_field(str, default = "py3"),
    },
)
