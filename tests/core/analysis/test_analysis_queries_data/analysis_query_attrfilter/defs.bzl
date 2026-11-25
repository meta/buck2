# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

def _labelled_impl(_ctx):
    return [DefaultInfo()]

# `+` on `labels` joins element-wise; on `text` it makes one string.
labelled = rule(
    impl = _labelled_impl,
    attrs = {
        "anything": attrs.any(default = None),
        "dep": attrs.option(attrs.dep(), default = None),
        "either": attrs.option(attrs.one_of(attrs.list(attrs.string()), attrs.string()), default = None),
        "flag": attrs.bool(default = False),
        "kinds": attrs.list(attrs.enum(["alpha", "beta"]), default = []),
        "labels": attrs.list(attrs.string(), default = []),
        "lists": attrs.option(attrs.one_of(attrs.list(attrs.string()), attrs.list(attrs.int())), default = None),
        "meta": attrs.dict(attrs.string(), attrs.string(), sorted = True, default = {}),
        "optional_labels": attrs.option(attrs.list(attrs.string()), default = None),
        "text": attrs.string(default = ""),
        "values": attrs.dict(attrs.string(), attrs.string(), default = {}),
    },
)

def _query_impl(ctx):
    return [
        DefaultInfo(
            default_output = ctx.actions.write(
                "out",
                ctx.attrs.arg,
                allow_args = True,
                has_content_based_path = False,
            )[0]
        )
    ]

query = rule(
    impl = _query_impl,
    attrs = {"arg": attrs.arg()},
)

def defs():
    constraint_setting(name = "choice")
    constraint_value(name = "one", constraint_setting = ":choice")
    config_setting(name = "is_one", constraint_values = [":one"])
    platform(name = "chosen", constraint_values = [":one"])
    platform(name = "other")

    labelled(
        name = "selected",
        labels = ["prefix"] + select({
            ":is_one": ["selected"],
            "DEFAULT": ["unselected"],
        }),
        text = "hello" + select({
            ":is_one": "world",
            "DEFAULT": "other",
        }),
    )
    labelled(name = "plain", labels = ["prefix"])

    # A string concat inside a list element: joined, like `text`.
    labelled(
        name = "nested",
        labels = [
            "hello" + select({
                ":is_one": "world",
                "DEFAULT": "other",
            })
        ],
    )

    # A dep renders with its configuration once configured.
    labelled(name = "with_dep", dep = ":plain")

    # Lists behind `select()`s and concats: an element that is a `select()`
    # with a string concat for a branch, a `select()` whose branch is a list
    # concat with another `select()` in it, an `option` list, an enum list
    # and an `any` list; a `select()` over something that is not a string;
    # and values that `one_of` or a dict keep from being read element by
    # element.
    labelled(
        name = "shapes",
        labels = [
            select({
                ":is_one": "item-"
                + select({
                    ":is_one": "chosen",
                    "DEFAULT": "unchosen",
                }),
                "DEFAULT": "item-other",
            })
        ] + select({
            ":is_one": ["sel-" + "chosen"]
            + ["sel-tail"]
            + select({
                ":is_one": ["inner-chosen"],
                "DEFAULT": ["inner-other"],
            }),
            "DEFAULT": ["sel-other"],
        }),
        optional_labels = ["opt"] + select({
            ":is_one": ["opt-chosen"],
            "DEFAULT": ["opt-other"],
        }),
        kinds = ["alpha"] + select({":is_one": ["beta"], "DEFAULT": []}),
        either = ["either-list"] + select({
            ":is_one": ["either-chosen"],
            "DEFAULT": ["either-other"],
        }),
        lists = ["lists"] + select({
            ":is_one": ["lists-chosen"],
            "DEFAULT": ["lists-other"],
        }),
        anything = ["any-string", 1],
        flag = select({":is_one": True, "DEFAULT": False}),
        values = {
            "k": select({
                ":is_one": "v-chosen",
                "DEFAULT": "v-other",
            })
        },
    )

    # A sorted dict concat is matched in sorted order: `a` before `z`, whose
    # value makes the regex below give up.
    labelled(
        name = "sorted_dict",
        # buildifier: disable=dict-concatenation
        meta = {"z": "a" * 80 + "!"} + select({"DEFAULT": {"a": "MATCH"}}),
    )

    # The test passes the queries it checks as `-c test.queries=<json>`.
    for name, expr in json.decode(read_config("test", "queries", "{}")).items():
        query(name = "q_" + name, arg = "$(query_targets {})".format(expr))
