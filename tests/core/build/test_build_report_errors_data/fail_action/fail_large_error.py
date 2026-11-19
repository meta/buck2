# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

import sys

# Generate a very large error message (more than 20KB threshold)
# The truncation threshold is 20 * 1024 = 20480 bytes
large_message = "ERROR: " + ("X" * 25000) + " END_MARKER"
print(large_message, file=sys.stderr)
sys.exit(1)
