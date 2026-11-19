# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

CxxGenericErrorType = record(
    matcher = str | BuckRegex,
    category_suffix = str,
)

def make_error_type(matcher = None, category_suffix = str) -> CxxGenericErrorType:
    return CxxGenericErrorType(
        matcher = matcher,
        category_suffix = category_suffix,
    )

def _match(matcher: str | BuckRegex, stderr: str) -> bool:
    if isinstance(matcher, str):
        return matcher in stderr
    elif isinstance(matcher, BuckRegex):
        return matcher.match(stderr)
    else:
        fail("Unknown matcher type: {}", type(matcher))

# Not applied automatically by the prelude; add them in your toolchain if you want them
#
# FIXME(JakobDegen): Change that?
def cxx_generic_error_handler(ctx: ActionErrorCtx) -> list[ActionSubError]:
    structured_error = ctx.parse_with_errorformat(
        category = "cxx_compiler_error",
        error = ctx.stderr,
        # reference partially: https://github.com/neovim/neovim/blob/07915941014307b054cbbb131369377647b7fb06/runtime/compiler/gcc.vim#L19-L37
        errorformats = [
            "%f:%l:%c:\\ %trror:\\ %m",
            "%f:%l:%c:\\ %tarning:\\ %m",
            "%f(%l,%c):\\ %trror:\\ %m",
            "%f(%l,%c):\\ %tarning:\\ %m",
            "%f:%l:\\ %trror:\\ %m",
            "%f:%l:\\ %tarning:\\ %m",
        ],
    )

    for e in structured_error:
        if e.error_type != None and e.error_type.lower() == "w":
            e.category = "cxx_compiler_warning"
        else:
            for error_type in CXX_GENERIC_ERROR_TYPES:
                if _match(error_type.matcher, e.message):
                    e.category = "cxx_{}".format(error_type.category_suffix)
                    break

    # If we didn't match any of the structured errors, try to get the category from the error message
    categories = []
    if len(structured_error) == 0:
        for error_type in CXX_GENERIC_ERROR_TYPES:
            if _match(error_type.matcher, ctx.stderr):
                categories.append(
                    ctx.new_sub_error(
                        category = "cxx_{}".format(error_type.category_suffix),
                    )
                )

    return structured_error + categories

# PLEASE READ BEFORE ADDING NEW ERROR TYPES!
#
# Error types added here should be generic across all cxx toolchains, toolchain/platform specific errors
# should be defined in a separate toolchain and provided accordingly, these generic errors will automatically
# be added to all types. If you wish to add messages, it must be applicable to ALL scenarios, including OSS.
#
# Matching is case-sensitive: write matchers the way the tool prints them.
CXX_GENERIC_ERROR_TYPES = [
    # linker errors
    make_error_type(
        matcher = "error: duplicate symbol",
        category_suffix = "linker_duplicate_symbol",
    ),
    make_error_type(
        matcher = "error: undefined symbol",
        category_suffix = "linker_undefined_symbol",
    ),
    make_error_type(
        matcher = "error: unable to find library",
        category_suffix = "linker_missing_library",
    ),
    make_error_type(
        matcher = "cannot be used against local symbol",
        category_suffix = "linker_relocation",
    ),
    # compilation errors
    make_error_type(
        matcher = "file not found",
        category_suffix = "file_not_found",
    ),
    make_error_type(
        matcher = "extra-semi",
        category_suffix = "extra_semicolon",
    ),
    make_error_type(
        matcher = "cannot open file",
        category_suffix = "cannot_open_file",
    ),
    make_error_type(
        matcher = "too few arguments to function call",
        category_suffix = "too_few_arguments",
    ),
    make_error_type(
        matcher = "use of undeclared identifier",
        category_suffix = "undeclared_identifier",
    ),
    make_error_type(
        matcher = "macro redefined",
        category_suffix = "macro_redefined",
    ),
    make_error_type(
        matcher = "no type named",
        category_suffix = "missing_type",
    ),
    make_error_type(
        matcher = "no template named",
        category_suffix = "missing_template",
    ),
    make_error_type(
        matcher = "no matching function for call",
        category_suffix = "missing_function",
    ),
    make_error_type(
        matcher = "a type specifier is required for all declarations",
        category_suffix = "type_specifier_required",
    ),
    make_error_type(
        matcher = "definition of implicit copy constructor for",
        category_suffix = "definition_deprecated",
    ),
    make_error_type(
        matcher = "Permission denied",
        category_suffix = "permission_denied",
    ),
    make_error_type(
        matcher = "incomplete type",
        category_suffix = "incomplete_type",
    ),
    make_error_type(
        matcher = "C2440",
        category_suffix = "type_conversion_failed",
    ),
    make_error_type(
        matcher = "cannot initialize a member subobject of type",
        category_suffix = "type_mismatch",
    ),
    make_error_type(
        matcher = "no matching member function",
        category_suffix = "missing_member_function",
    ),
    make_error_type(
        matcher = "unused variable",
        category_suffix = "unused_variable",
    ),
    make_error_type(
        matcher = "Wheader-hygiene",
        category_suffix = "header_hygiene_violated",
    ),
    make_error_type(
        matcher = "call to non-static member function",
        category_suffix = "non_static_member_function_call",
    ),
    make_error_type(
        matcher = "too many template arguments for class template",
        category_suffix = "too_many_template_arguments",
    ),
    make_error_type(
        matcher = "possible misuse of comma operator",
        category_suffix = "comma_operator_misuse",
    ),
    make_error_type(
        matcher = regex("(?i)no such file or directory"),
        category_suffix = "no_such_file_or_directory",
    ),
    make_error_type(
        matcher = "Wpadded",
        category_suffix = "padding_error",
    ),
    # compiler errors matched by regex
    make_error_type(
        matcher = regex("no member named '.+' in"),
        category_suffix = "missing_member",
    ),
    make_error_type(
        matcher = regex("reference to '.+' is ambiguous"),
        category_suffix = "ambiguous_reference",
    ),
    make_error_type(
        matcher = regex("use of overloaded operator '.+' is ambiguous"),
        category_suffix = "ambiguous_overload_operator",
    ),
    make_error_type(
        matcher = regex("call to constructor of '.+' is ambiguous"),
        category_suffix = "ambiguous_constructor_call",
    ),
]
