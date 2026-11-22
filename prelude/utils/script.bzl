# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

def write_script(
    actions: AnalysisActions,
    name: str,
    lines: list,
    language: str,
    has_content_based_path: bool = False,
) -> cmd_args:
    if language == "sh":
        runner = ["/usr/bin/env", "bash", "-e"]
    elif language == "bat":
        runner = ["cmd.exe", "/v:off", "/c"]
    else:
        fail("Unsupported script language: {}".format(language))
    script, _ = actions.write(
        "{}.{}".format(name, language),
        lines,
        is_executable = True,
        allow_args = True,
        has_content_based_path = has_content_based_path,
    )
    return cmd_args(runner, script, hidden = lines)
