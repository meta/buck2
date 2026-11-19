# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

"""
Type definitions for the unified error enrichment framework.
"""

ErrorEnricher = record(
    # Pattern to match against the error message.
    # str performs a case-sensitive substring match.
    matcher = str | BuckRegex,
    # If specified, matcher will only run if the filepath contains this string.
    file_matcher = field([str, None], default = None),
    # Category to set on the error when matched.
    category = str,
    # Optional remediation message to help users fix the error.
    # This is a general hint that applies to all matches of this pattern.
    message = field([str, None], default = None),
    # Optional function to extract a subcategory from the error text.
    # This enables more granular categorization within a category.
    subcategory_extractor = field([typing.Callable[[str], str | None], None], default = None),
    # Map from subcategory to specific remediation message.
    # When a subcategory is extracted, this map provides targeted remediation.
    subcategory_remediations = field([dict[str, str], None], default = None),
)
