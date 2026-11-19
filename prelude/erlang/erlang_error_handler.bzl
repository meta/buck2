# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

load(
    ":erlang_info.bzl",
    "ErlangErrorHandlers",
)

# erlc error format:
#   The erlc compiler error format is as following:
#     <file>:<line>:<column>: <message>

def erlang_erlc_action_error_handler(ctx: ActionErrorCtx) -> list[ActionSubError]:
    structured_errors = ctx.parse_with_errorformat(
        category = "erlang_compile_error",
        error = ctx.stderr,
        errorformats = [
            "%f:%l:%c:\\ %m",
        ],
    )

    return structured_errors

# extract_otp_app error format:
#     <file>:<line>:<error_number>: <message>
#
# NOTE: <message> is JSON encoded

def erlang_extract_otp_app_error_handler(ctx: ActionErrorCtx) -> list[ActionSubError]:
    parsed_errors = ctx.parse_with_errorformat(
        category = "extract_from_otp_error",
        error = ctx.stderr,
        errorformats = [
            "%f:%l:%n:\\ %m",
        ],
    )

    structured_errors = []

    if parsed_errors:
        structured_errors.extend(parsed_errors)
        for parsed_error in parsed_errors:
            if parsed_error.error_number == 1 and parsed_error.message != None:
                # otp_application_not_found
                error_json = json.decode(parsed_error.message)
                app_name = error_json["name"]

                # Build available applications list if present
                available_apps_section = ""
                if "available_applications" in error_json and error_json["available_applications"]:
                    available_apps = error_json["available_applications"]
                    apps_list = "\n".join(["   - {}".format(app) for app in available_apps])
                    available_apps_section = "\n\n Available OTP applications ({count}):\n{apps_list}".format(count = len(available_apps), apps_list = apps_list)

                remediation = """To fix, verify '{app_name}' exists in your OTP installation:

    $ ls -Al "{root_dir}/lib/{app_name}-*"

 Common causes:
   - The application is not part of your OTP version (e.g., jinterface was removed in OTP 27)
   - You're using a toolchain configured for a different OTP version{available_apps_section}""".format(
                    app_name = app_name,
                    root_dir = error_json["root_dir"],
                    available_apps_section = available_apps_section,
                )
                structured_errors.append(
                    ctx.new_sub_error(
                        category = parsed_error.category,
                        subcategory = "otp_application_not_found",
                        message = error_json["message"],
                        remediation = remediation,
                        file = parsed_error.file,
                        lnum = parsed_error.lnum,
                        error_number = parsed_error.error_number,
                        show_in_stderr = True,
                    )
                )
            elif parsed_error.error_number == 2 and parsed_error.message != None:
                # no_matches_for_wildcard
                error_json = json.decode(parsed_error.message)
                structured_errors.append(
                    ctx.new_sub_error(
                        category = parsed_error.category,
                        subcategory = "no_matches_for_wildcard",
                        message = error_json["message"],
                        file = parsed_error.file,
                        lnum = parsed_error.lnum,
                        error_number = parsed_error.error_number,
                        show_in_stderr = True,
                    )
                )
            elif parsed_error.error_number == 3 and parsed_error.message != None:
                # multiple_matches_for_wildcard
                error_json = json.decode(parsed_error.message)
                structured_errors.append(
                    ctx.new_sub_error(
                        category = parsed_error.category,
                        subcategory = "multiple_matches_for_wildcard",
                        message = error_json["message"],
                        file = parsed_error.file,
                        lnum = parsed_error.lnum,
                        error_number = parsed_error.error_number,
                        show_in_stderr = True,
                    )
                )

    return structured_errors

erlang_action_error_handler = ErlangErrorHandlers(
    erlc = erlang_erlc_action_error_handler,
)
