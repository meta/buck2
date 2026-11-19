# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

"""
Unified error enrichment framework for Buck2 structured build errors.

This module provides a shared abstraction for matching and enriching build errors
across different languages.
"""

load("@prelude//error_handler:error_enricher_types.bzl", "ErrorEnricher")

def match_error(matcher: str | BuckRegex, text: str) -> bool:
    if isinstance(matcher, str):
        return matcher in text
    elif isinstance(matcher, BuckRegex):
        return matcher.match(text)
    else:
        fail("Unknown matcher type: {}".format(type(matcher)))

def _apply_subcategory_remediation(error: ActionSubError, enricher: ErrorEnricher, text: str):
    """
    Extracts an error subcategory (if configured) and applies the remediation
    specific to that subcategory. Subcategories let you target tailored fixes
    for specific error objects within a broader category.
    """
    if enricher.subcategory_extractor:
        subcategory = enricher.subcategory_extractor(text)
        if subcategory:
            error.subcategory = subcategory
            if enricher.subcategory_remediations and subcategory in enricher.subcategory_remediations:
                error.remediation = enricher.subcategory_remediations[subcategory]

def enrich_errors(errors: list[ActionSubError], enrichers: list[ErrorEnricher], category_prefix: str = "") -> list[ActionSubError]:
    for error in errors:
        message = error.message if error.message else ""
        path = error.file if error.file else ""

        for enricher in enrichers:
            if enricher.file_matcher and enricher.file_matcher not in path:
                continue
            if not match_error(enricher.matcher, message):
                continue

            error.category = category_prefix + enricher.category
            _apply_subcategory_remediation(error, enricher, message)
            if not error.remediation and enricher.message:
                error.remediation = enricher.message
            break

    return errors

def create_and_enrich_errors(ctx: ActionErrorCtx, text: str, enrichers: list[ErrorEnricher], category_prefix: str = "") -> list[ActionSubError]:
    """
    Create an ActionSubError for EACH matching enricher.

    Unlike enrich_errors() which enriches existing errors with the first match,
    this function creates new errors for every matching enricher pattern.

    Use this for stderr-based error handlers where multiple categories
    may match the same output.
    """
    errors = []
    for enricher in enrichers:
        if not match_error(enricher.matcher, text):
            continue

        # Create new error for this match
        error = ctx.new_sub_error(
            category = category_prefix + enricher.category,
            message = enricher.message,
        )

        _apply_subcategory_remediation(error, enricher, text)
        errors.append(error)

    return errors
