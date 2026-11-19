# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

# A test of various types of attribute

MirrorInfo = provider(fields = ["info"])

def _assert_eq(a, b):
    if a != b:
        fail("Expected {} == {}".format(a, b))

# Test primitives

def _mirror_impl(ctx: AnalysisContext) -> list[Provider]:
    return [DefaultInfo(), MirrorInfo(info = ctx.attrs)]

_mirror = rule(
    impl = _mirror_impl,
    attrs = {
        "defaulted": attrs.string(default = "a-default"),
        "enum": attrs.enum(["red", "green", "blue"]),
        "false": attrs.bool(),
        "int": attrs.int(),
        "list_string": attrs.list(attrs.string()),
        "string": attrs.string(),
        "true": attrs.bool(),
    },
)

def _simple_impl(ctx: AnalysisContext) -> Promise:
    def f(providers):
        res = providers[MirrorInfo].info
        _assert_eq(res.true, True)
        _assert_eq(res.false, False)
        _assert_eq(res.int, 42)
        _assert_eq(res.string, "a-string")
        _assert_eq(res.list_string, ["a", "b", "c"])
        _assert_eq(res.defaulted, "a-default")
        _assert_eq(res.enum, "red")
        return [DefaultInfo()]

    at = {
        "enum": "red",
        "false": False,
        "int": 42,
        "list_string": ["a", "b", "c"],
        "name": ctx.label,
        "string": "a-string",
        "true": True,
    }
    return ctx.actions.anon_target(_mirror, at).promise.map(f)

_simple = rule(impl = _simple_impl, attrs = {})

# Test dep

_mirror2 = rule(
    impl = _mirror_impl,
    attrs = {
        "dep": attrs.dep(),
    },
)

def _complex_impl(ctx: AnalysisContext) -> Promise:
    def f(providers):
        res = providers[MirrorInfo].info
        _assert_eq(res.dep[DefaultInfo].default_outputs[0].short_path, "my_short_path")
        return [DefaultInfo()]

    return ctx.actions.anon_target(_mirror2, {"dep": ctx.attrs.dep}).promise.map(f)

_complex = rule(
    impl = _complex_impl,
    attrs = {
        "dep": attrs.dep(default = "//anon:attributes_complex_source"),
    },
)

# Test collections

_mirror3 = rule(
    impl = _mirror_impl,
    attrs = {
        "deps": attrs.list(attrs.dep()),
        "dict": attrs.dict(key = attrs.string(), value = attrs.dep()),
        "one_of": attrs.one_of(attrs.dep(), attrs.bool()),
        "set": attrs.set(attrs.dep()),
        "tuple": attrs.tuple(attrs.dep(), attrs.string()),
    },
)

def _complex_collection_impl(ctx: AnalysisContext) -> Promise:
    def f(providers):
        res = providers[MirrorInfo].info
        _assert_eq(res.deps[0][DefaultInfo].default_outputs[0].short_path, "my_short_path")
        _assert_eq(res.tuple[0][DefaultInfo].default_outputs[0].short_path, "my_short_path")
        _assert_eq(res.tuple[1], "my_string")
        _assert_eq(res.dict["my_key"][DefaultInfo].default_outputs[0].short_path, "my_short_path")
        _assert_eq(res.one_of[DefaultInfo].default_outputs[0].short_path, "my_short_path")
        _assert_eq(res.set[0][DefaultInfo].default_outputs[0].short_path, "my_short_path")
        return [DefaultInfo()]

    return ctx.actions.anon_target(
        _mirror3,
        {
            "deps": ctx.attrs.deps,
            "dict": ctx.attrs.dict,
            "one_of": ctx.attrs.one_of,
            "set": ctx.attrs.set,
            "tuple": ctx.attrs.tuple,
        },
    ).promise.map(f)

_complex_collection = rule(
    impl = _complex_collection_impl,
    attrs = {
        "deps": attrs.list(attrs.dep(), default = ["//anon:attributes_complex_source"]),
        "dict": attrs.dict(key = attrs.string(), value = attrs.dep(), default = {"my_key": "//anon:attributes_complex_source"}),
        "one_of": attrs.one_of(attrs.dep(), attrs.bool(), default = "//anon:attributes_complex_source"),
        "set": attrs.set(attrs.dep(), default = ["//anon:attributes_complex_source"]),
        "tuple": attrs.tuple(attrs.dep(), attrs.string(), default = ("//anon:attributes_complex_source", "my_string")),
    },
)

def _complex_source_impl(ctx: AnalysisContext) -> list[Provider]:
    artifact = ctx.actions.write("my_short_path", "", has_content_based_path = False)
    return [DefaultInfo(default_output = artifact)]

_complex_source = rule(impl = _complex_source_impl, attrs = {})

# Test artifacts

_artifacts_mirror = rule(
    impl = _mirror_impl,
    attrs = {
        "build_artifact": attrs.source(),
        "declared_artifact": attrs.source(),
        "source_artifact": attrs.source(),
    },
)

def _complex_artifacts_impl(ctx: AnalysisContext) -> Promise:
    def f(providers):
        res = providers[MirrorInfo].info
        _assert_eq(res.source_artifact.is_source, True)
        _assert_eq(res.build_artifact.is_source, False)
        _assert_eq(res.declared_artifact.is_source, False)
        _assert_eq(res.source_artifact.basename, "my_source")
        _assert_eq(res.build_artifact.basename, "my_short_path")
        _assert_eq(res.declared_artifact.basename, "my_shorter_path")
        return [DefaultInfo()]

    declared_artifact = ctx.actions.write("my_shorter_path", "", has_content_based_path = False)

    return ctx.actions.anon_target(
        _artifacts_mirror,
        {
            "build_artifact": ctx.attrs.build_artifact,
            "declared_artifact": declared_artifact,
            "source_artifact": ctx.attrs.source_artifact,
        },
    ).promise.map(f)

_complex_artifacts = rule(
    impl = _complex_artifacts_impl,
    attrs = {
        "build_artifact": attrs.source(default = "//anon:attributes_complex_source"),
        "source_artifact": attrs.source(),
    },
)

# Test promise_artifacts

HelloInfo = provider(fields = ["output"])

def _builder_impl(ctx: AnalysisContext) -> list[Provider]:
    hello = ctx.actions.write("hello.out", "hello", has_content_based_path = False)
    return [DefaultInfo(), HelloInfo(output = hello)]

_builder = anon_rule(impl = _builder_impl, artifact_promise_mappings = {"artifact": lambda x: x[HelloInfo].output}, attrs = {})

def _promise_artifact_mirror_impl(ctx: AnalysisContext) -> list[Provider]:
    out = ctx.actions.declare_output("output", has_content_based_path = False)
    ctx.actions.run(["cp", ctx.attrs.promise_artifact, out.as_output()], category = "cp")
    return [DefaultInfo(default_output = out), MirrorInfo(info = ctx.attrs)]

_promise_artifact_mirror = rule(
    impl = _promise_artifact_mirror_impl,
    attrs = {
        "promise_artifact": attrs.source(),
    },
)

def _promise_artifact_impl(ctx: AnalysisContext) -> Promise:
    def f(providers):
        res = providers[MirrorInfo].info
        _assert_eq(res.promise_artifact.is_source, False)
        return [providers[DefaultInfo]]

    promise_artifact = ctx.actions.anon_target(_builder, {}).artifact("artifact")

    return ctx.actions.anon_target(
        _promise_artifact_mirror,
        {
            "promise_artifact": promise_artifact,
        },
    ).promise.map(f)

_promise_artifact = rule(impl = _promise_artifact_impl, attrs = {})

def _check_impl(ctx: AnalysisContext) -> list[Provider]:
    out = ctx.actions.declare_output("output", has_content_based_path = False)

    def f(ctx: AnalysisContext, artifacts, outputs):
        _assert_eq(artifacts[ctx.attrs.src].read_string(), "hello")
        ctx.actions.write(outputs[out], "")

    ctx.actions.dynamic_output(dynamic = [ctx.attrs.src], inputs = [], outputs = [out.as_output()], f = f)
    return [DefaultInfo(default_output = out)]

_check = rule(impl = _check_impl, attrs = {"src": attrs.source()})

# Test label

LabelTestInfo = provider(fields = ["info"])

def _rule_with_subtarget_impl(ctx: AnalysisContext) -> list[Provider]:
    return [DefaultInfo(), LabelTestInfo(info = ctx.attrs)]

_rule_with_subtarget = rule(
    impl = _rule_with_subtarget_impl,
    attrs = {
        "my_string": attrs.string(default = "a-string"),
    },
)

_label_mirror = rule(
    impl = _mirror_impl,
    attrs = {
        "subtarget_label": attrs.label(),
        "target_label": attrs.label(),
    },
)

def _label_impl(ctx: AnalysisContext) -> Promise:
    def f(providers):
        target_label = providers[MirrorInfo].info.target_label
        subtarget_label = providers[MirrorInfo].info.subtarget_label
        _assert_eq(isinstance(subtarget_label, ProvidersLabel), True)
        _assert_eq(subtarget_label.cell, "root")
        _assert_eq(subtarget_label.name, "rule_with_subtarget")
        _assert_eq(isinstance(target_label, ProvidersLabel), True)
        _assert_eq(target_label.cell, "root")
        return [DefaultInfo()]

    return ctx.actions.anon_target(
        _label_mirror,
        {
            # Test that we can pass in an unconfigured subtarget label or an unconfigured target label
            # ctx.attrs.label is a configured subtarget label at this point, so we can do some magic here
            # to get the underlying unconfigured subtarget label via `with_sub_target()`, and we can also
            # call `raw_target()` to get the underlying unconfigured target label. We do not accept
            # configured labels for anon targets because anon targets do not support configurations in general.
            "subtarget_label": ctx.attrs.label.raw_target().with_sub_target("LabelTestInfo"),
            "target_label": ctx.attrs.label.raw_target(),
        },
    ).promise.map(f)

_label = rule(
    impl = _label_impl,
    attrs = {
        "label": attrs.label(default = "//anon:rule_with_subtarget"),
    },
)

# Create targets for the tests

def attributes_test():
    _simple(name = "attributes_simple")
    _complex(name = "attributes_complex")
    _complex_collection(name = "attributes_complex_collection")
    _complex_source(name = "attributes_complex_source")
    _complex_artifacts(
        name = "attributes_complex_artifacts",
        source_artifact = "my_source",
    )

    _promise_artifact(name = "promise_artifact")
    _check(name = "check", src = ":promise_artifact")

    _rule_with_subtarget(name = "rule_with_subtarget")
    _label(name = "label")
