# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

"""
Handle labels used to make genrules prefer local execution
"""

# Some rules prefer to be run locally for various reasons listed next to the label.
_GENRULE_PREFER_LOCAL_LABELS = set([
    # Used for rules that just copy large files and will be faster to do locally
    "large_copy",
])

def resolved_genrule_labels_prefer_local(labels: list[str]) -> bool:
    for label in labels:
        if label in _GENRULE_PREFER_LOCAL_LABELS:
            return True
    return False
