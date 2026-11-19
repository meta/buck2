# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

load("@prelude//:prelude_rule.bzl", "prelude_rule")
load("@prelude//decls:common.bzl", "buck")
load(":android_providers.bzl", "KeystoreInfo")

def keystore_impl(ctx: AnalysisContext) -> list[Provider]:
    sub_targets = {}
    sub_targets["keystore"] = [DefaultInfo(default_output = ctx.attrs.store)]
    sub_targets["properties"] = [DefaultInfo(default_output = ctx.attrs.properties)]

    return [
        KeystoreInfo(store = ctx.attrs.store, properties = ctx.attrs.properties),
        DefaultInfo(default_outputs = [ctx.attrs.store, ctx.attrs.properties], sub_targets = sub_targets),
    ]

keystore = prelude_rule(
    name = "keystore",
    impl = keystore_impl,
    docs = """
        A `keystore()` contains the data for a key pair
        created by the `keytool` executable that comes
        with the JDK. This is a required input for an `android_binary()` rule.
    """,
    examples = None,
    further = None,
    attrs = (
        # @unsorted-dict-items
        {
            "store": attrs.source(
                doc = """
                The path to the file that contains the key.
                 This is the path that was passed as the `-keystore` argument
                 when you ran `keytool`.
            """,
            ),
            "properties": attrs.source(
                doc = """
                The path to the `.properties` file that contains the following values:

                ```
                # The value that you passed as the argument to -alias
                # when you ran keytool.
                key.alias=my_alias

                # The value that you entered in response to
                # the "Enter keystore password:" prompt.
                key.store.password=store_password

                # The value that you entered in response to
                # the "Enter key password for <my_alias>" prompt.
                key.alias.password=alias_password
                ```
            """,
            ),
            "deps": attrs.list(attrs.dep(), default = []),
        }
        | buck.licenses_arg()
        | buck.labels_arg()
        | buck.contacts_arg()
    ),
)
