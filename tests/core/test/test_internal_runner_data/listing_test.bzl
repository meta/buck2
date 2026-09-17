# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

def _parse_test_listing(listing_content: str) -> list[dict[str, str]]:
    entries = []
    for name in listing_content.split():
        entry = {"filter": name, "name": name}
        if name.startswith("skip_"):
            entry["message"] = "skipped by listing"
            entry["status"] = "SKIP"
        entries.append(entry)
    return entries

def _parse_test_result(stdout: str, _stderr: str, exit_code: int) -> list[dict]:
    status = "PASS" if exit_code == 0 else "FAIL"
    return [{"name": stdout.strip(), "status": status}]

def _impl(ctx: AnalysisContext) -> list[Provider]:
    return [
        DefaultInfo(),
        InternalRunnerTestInfo(
            type = "custom",
            listing_command = ["fbpython", "-c", "print('" + " ".join(ctx.attrs.test_names) + "')"],
            command = ["fbpython", "-c", ctx.attrs.script],
            parse_test_listing = _parse_test_listing,
            parse_test_result = _parse_test_result,
        ),
    ]

listing_test = rule(
    impl = _impl,
    attrs = {
        "script": attrs.string(),
        "test_names": attrs.list(attrs.string()),
    },
)
