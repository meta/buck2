# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

# @oss-disable[end= ]: load(
    # @oss-disable[end= ]: "@prelude//apple/meta_only:apple_extra_error_categories.bzl",
    # @oss-disable[end= ]: "APPLE_CXX_FLAG_MESSAGES",
    # @oss-disable[end= ]: "APPLE_CXX_MESSAGE_CATEGORIES",
    # @oss-disable[end= ]: "APPLE_CXX_STDERR_CATEGORIES",
    # @oss-disable[end= ]: "APPLE_META_STDERR_ERROR_CATEGORIES",
    # @oss-disable[end= ]: "SWIFT_CATEGORY_REMEDIATION",
    # @oss-disable[end= ]: "SWIFT_STDERR_CATEGORIES",
# @oss-disable[end= ]: )
load("@prelude//apple/swift:swift_toolchain.bzl", "get_swift_toolchain_info")
load("@prelude//cxx:cxx_context.bzl", "get_cxx_toolchain_info")
load("@prelude//error_handler:error_enricher_types.bzl", "ErrorEnricher")

APPLE_CXX_FLAG_MESSAGES = {} # @oss-enable
APPLE_CXX_MESSAGE_CATEGORIES = [] # @oss-enable
APPLE_CXX_STDERR_CATEGORIES = [] # @oss-enable
APPLE_META_STDERR_ERROR_CATEGORIES = [] # @oss-enable
SWIFT_CATEGORY_REMEDIATION = {} # @oss-enable
SWIFT_STDERR_CATEGORIES = [] # @oss-enable

_APPLE_STDERR_ERROR_CATEGORIES = [
    # STOP! @oss-disable
    # If you work at Meta, you probably want to include things in APPLE_META_STDERR_ERROR_CATEGORIES   @oss-disable
    # so you can include a link to an internal resource (wiki, task, etc)                              @oss-disable
    # I would only add additional categories here if you think someone in open-source would benefit    @oss-disable
    # codesigning issues
    ErrorEnricher(matcher = "CodeSignProvisioningError", category = "code_sign_error"), # @oss-enable
    ErrorEnricher(matcher = "The timestamp service is not available", category = "code_sign_error"),
    # compilation issues
    ErrorEnricher(matcher = "failed to emit precompiled module", category = "pcm_compilation_failure"),
    ErrorEnricher(matcher = "please rebuild precompiled header", category = "pcm_compilation_failure"),
    ErrorEnricher(matcher = "llvm-lipo", category = "lipo_failure"),
    ErrorEnricher(matcher = ".modulemap:", category = "modulemap_compilation_failure"),
    ErrorEnricher(matcher = "missing required modules", category = "missing_required_modules_error"),
    ErrorEnricher(matcher = "has a minimum deployment target", category = "deployment_target_error"),
    # toolchain / genrule issues
    ErrorEnricher(matcher = "Stack dump:", category = "binary_execution_failure"),
    ErrorEnricher(matcher = "thread 'main' panicked", category = "binary_execution_failure"),
    ErrorEnricher(matcher = "error while loading shared libraries", category = "binary_execution_failure"),
    ErrorEnricher(matcher = "Traceback (most recent call last)", category = "python_execution_failure"),
    ErrorEnricher(matcher = "command not found", category = "command_not_found_failure"),
    ErrorEnricher(matcher = regex("(?i)command timed out"), category = "timeout_failure"),
    ErrorEnricher(matcher = regex("(?i)no such file or directory"), category = "no_such_file_failure"),
    # user errors
    ErrorEnricher(matcher = "Unknown target", category = "unknown_buck_target_failure"),
    # buck configuration issues
    ErrorEnricher(matcher = "unknown cell alias", category = "unknown_cell_alias_failure"),
]

def _match(matcher: str | BuckRegex, stderr: str) -> bool:
    if isinstance(matcher, str):
        return matcher in stderr
    elif isinstance(matcher, BuckRegex):
        return matcher.match(stderr)
    else:
        fail("Unknown matcher type: {}", type(matcher))

def _add_category_strings(ctx: ActionErrorCtx, stderr: str, errors: list[ActionSubError], source: list[ErrorEnricher]):
    for enricher in source:
        # Stderr carries no file path, so a file-scoped enricher cannot be honoured here.
        if enricher.file_matcher or not _match(enricher.matcher, stderr):
            continue

        subcategory = None
        remediation = enricher.message
        if enricher.subcategory_extractor:
            subcategory = enricher.subcategory_extractor(stderr)
            if subcategory and enricher.subcategory_remediations and subcategory in enricher.subcategory_remediations:
                remediation = enricher.subcategory_remediations[subcategory]

        errors.append(
            ctx.new_sub_error(
                category = "apple_" + enricher.category,
                subcategory = subcategory,
                remediation = remediation,
                show_in_stderr = remediation != None,
            )
        )

def _category_match(message: str, path: str, categories: list[ErrorEnricher]) -> ErrorEnricher | None:
    for error_category in categories:
        if error_category.file_matcher and error_category.file_matcher not in path:
            continue

        if _match(error_category.matcher, message):
            return error_category

    return None

def apple_build_error_handler(ctx: ActionErrorCtx) -> list[ActionSubError]:
    errors = []

    _add_category_strings(ctx, ctx.stderr, errors, _APPLE_STDERR_ERROR_CATEGORIES)
    _add_category_strings(ctx, ctx.stderr, errors, APPLE_META_STDERR_ERROR_CATEGORIES)

    return errors

def cxx_error_deserializer(ctx: AnalysisContext) -> RunInfo | None:
    return get_cxx_toolchain_info(ctx).binary_utilities_info.custom_tools.get("serialized-diags-to-json", None)

def swift_error_deserializer(ctx: AnalysisContext) -> RunInfo | None:
    return get_swift_toolchain_info(ctx).serialized_diags_to_json

def _valid_error(error: dict) -> bool:
    return "path" in error and "line" in error and "message" in error and "severity" in error

def cxx_error_handler(ctx: ActionErrorCtx) -> list[ActionSubError]:
    errors = []
    for val in ctx.output_artifacts.values():
        for error_json in val.read_json():
            if not _valid_error(error_json):
                continue

            # Clang optionally populates a category and flag field. We are only
            # interested in the flag for now, which helps to know which flag to
            # disable to bypass an error.
            remediation = None
            postfix = ""
            subcategory = None
            error_flag = error_json.get("flag", None)
            if error_flag:
                subcategory = error_flag.replace("-", "_")
                remediation = APPLE_CXX_FLAG_MESSAGES.get(error_flag, None)
                postfix = " [-W{}]".format(error_flag)

            # Fall back to per-error message matching when no flag-based
            # remediation matched, mirroring swift_error_handler.
            if remediation == None:
                custom_category = _category_match(
                    message = error_json["message"],
                    path = error_json["path"],
                    categories = APPLE_CXX_MESSAGE_CATEGORIES,
                )
                if custom_category:
                    subcategory = custom_category.category
                    if custom_category.message:
                        remediation = custom_category.message

            errors.append(
                ctx.new_sub_error(
                    category = "apple_cxx_" + error_json["severity"],
                    message = error_json["message"] + postfix,
                    file = error_json["path"],
                    lnum = error_json["line"],
                    col = error_json["col"],
                    remediation = remediation,
                    show_in_stderr = remediation != None,
                    subcategory = subcategory,
                ),
            )

    # The cxx error handler is also used for linking, which will not have any
    # serialzed diagnostics, so go through the stderr matcher here.
    if not ctx.output_artifacts.values():
        _add_category_strings(ctx, ctx.stderr, errors, APPLE_CXX_STDERR_CATEGORIES)

    return errors

def _get_error_type(severity: str) -> str | None:
    error_type = None
    if severity == "error":
        error_type = "E"
    elif severity == "warning":
        error_type = "W"
    elif severity == "note":
        error_type = "N"

    return error_type

def swift_error_handler(ctx: ActionErrorCtx) -> list[ActionSubError]:
    errors = []
    for val in ctx.output_artifacts.values():
        for error_json in val.read_json():
            if not _valid_error(error_json):
                continue

            severity = error_json["severity"]
            message = error_json["message"]
            remediation = None
            subcategory = None

            # Swift serializes the category in the form:
            # SendableClosureCaptures@https://docs.swift.org/compiler/documentation/diagnostics/sendable-closure-captures
            category = error_json.get("category", "")
            if "@" in category:
                # Convert to markdown links for phabricator
                components = error_json["category"].split("@")
                message += " [{}]({})".format(components[0], components[1])
                subcategory = components[0].lower()
                if components[0] in SWIFT_CATEGORY_REMEDIATION:
                    remediation = SWIFT_CATEGORY_REMEDIATION[components[0]]
            elif category in SWIFT_CATEGORY_REMEDIATION:
                remediation = SWIFT_CATEGORY_REMEDIATION[category]
            else:
                # With no category in the error itself we categorise based on
                # the message content.
                custom_category = _category_match(
                    message = message,
                    path = error_json["path"],
                    categories = SWIFT_STDERR_CATEGORIES,
                )
                if custom_category:
                    subcategory = custom_category.category
                    if custom_category.message:
                        remediation = custom_category.message
            errors.append(
                ctx.new_sub_error(
                    category = "swift_" + severity,
                    message = message,
                    file = error_json["path"],
                    lnum = error_json["line"],
                    col = error_json["col"],
                    error_type = _get_error_type(severity),
                    remediation = remediation,
                    show_in_stderr = remediation != None,
                    subcategory = subcategory,
                ),
            )

    return errors
