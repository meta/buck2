# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

load(":toolchains_common.bzl", "toolchains_common")

def _opts_for_tests_arg() -> Attr:
    # Attributes types do not have records.
    # The expected shape of re_opts is:
    # {
    #     "capabilities": Dict<str, str> | None
    #     "listing_capabilities": Dict<str, str> | None
    #     "local_listing_enabled": bool | None
    #     "disable_local_listing": bool | None  # Overrides local_listing_enabled.
    #     "local_enabled": bool |  None
    #     "remote_enabled": bool | None
    #     "use_case": str | None
    #     "remote_cache_enabled": bool | None
    #     "dependencies": list<Dict<str, str>> | []
    #     "gang_workers": list<Dict<str, str>> | []
    #     "gang": Dict<str, str | int | Dict<str, str>> | None
    #     "resource_units": int | None
    #     "remote_execution_dynamic_image": dict<str, str | list<str>> | None
    # }
    return attrs.dict(
        key = attrs.string(),
        value = attrs.option(
            attrs.one_of(
                attrs.dict(
                    key = attrs.string(),
                    value = attrs.one_of(
                        attrs.string(),
                        attrs.list(attrs.string()),
                        attrs.dict(key = attrs.string(), value = attrs.string(), sorted = False),
                        attrs.int(),
                    ),
                    sorted = False,
                ),
                attrs.string(),
                attrs.bool(),
                attrs.list(attrs.dict(key = attrs.string(), value = attrs.string()), default = []),
                attrs.int(),
            ),
            # TODO(cjhopman): I think this default does nothing, it should be deleted
            default = None,
        ),
        sorted = False,
    )

def _test_args() -> dict[str, Attr]:
    return {
        "remote_execution": attrs.option(
            attrs.one_of(
                attrs.string(),
                _opts_for_tests_arg(),
            ),
            default = None,
        ),
        "_remote_test_execution_toolchain": toolchains_common.remote_test_execution(),
    }

re_test_common = struct(
    test_args = _test_args,
    opts_for_tests_arg = _opts_for_tests_arg,
)
