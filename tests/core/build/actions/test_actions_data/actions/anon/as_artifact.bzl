# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

# A very basic test that two things can share a single anon target

def _assert_eq(a, b):
    if a != b:
        fail("Expected {} == {}".format(a, b))

HelloInfo = provider(fields = ["output"])

def _builder_impl(ctx: AnalysisContext) -> list[Provider]:
    hello = ctx.actions.write("dir/hello.out", "hello", has_content_based_path = False)
    return [DefaultInfo(), HelloInfo(output = hello)]

_builder = anon_rule(
    impl = _builder_impl,
    attrs = {},
    artifact_promise_mappings = {
        "artifact": lambda x: x[HelloInfo].output,
    },
)

def _build_impl(ctx: AnalysisContext) -> list[Provider]:
    anon_target = ctx.actions.anon_target(_builder, {})
    artifact = anon_target.artifact("artifact")
    artifact_from_dict = anon_target.artifacts()["artifact"]
    _assert_eq(artifact, artifact_from_dict)
    out = ctx.actions.declare_output("output", has_content_based_path = False)
    ctx.actions.run(["cp", artifact, out.as_output()], category = "cp")
    return [DefaultInfo(default_output = out)]

_build = rule(impl = _build_impl, attrs = {})

def _check_impl(ctx: AnalysisContext) -> list[Provider]:
    out = ctx.actions.declare_output("output", has_content_based_path = False)

    def f(ctx: AnalysisContext, artifacts, outputs):
        _assert_eq(artifacts[ctx.attrs.src].read_string(), "hello")
        ctx.actions.write(outputs[out], "")

    ctx.actions.dynamic_output(dynamic = [ctx.attrs.src], inputs = [], outputs = [out.as_output()], f = f)
    return [DefaultInfo(default_output = out)]

_check = rule(impl = _check_impl, attrs = {"src": attrs.source()})

def _short_path_impl(ctx: AnalysisContext) -> list[Provider]:
    artifact = ctx.actions.anon_target(_builder, {}).artifact("artifact")
    artifact_with_path = ctx.actions.assert_short_path(artifact, short_path = "dir/hello.out")
    _assert_eq(artifact_with_path.short_path, "dir/hello.out")
    _assert_eq(artifact_with_path.basename, "hello.out")
    _assert_eq(artifact_with_path.extension, ".out")
    return [DefaultInfo()]

_short_path = rule(impl = _short_path_impl, attrs = {})

# Test symlinked dir can accept a promise artifact

def _check_symlink_files_impl(ctx):
    artifact = ctx.actions.anon_target(_builder, {}).artifact("artifact")
    srcs = {"hello": artifact}

    out = ctx.actions.symlinked_dir("out", srcs, has_content_based_path = False)
    return [DefaultInfo(default_output = out)]

_check_symlink_files = rule(
    impl = _check_symlink_files_impl,
    attrs = {},
)

# Test copy can accept a promise artifact

def _check_copy_impl(ctx):
    artifact = ctx.actions.anon_target(_builder, {}).artifact("artifact")
    out = ctx.actions.copy_file("copied", artifact, has_content_based_path = False)
    return [DefaultInfo(default_output = out)]

_check_copy = rule(
    impl = _check_copy_impl,
    attrs = {},
)

# Test passing in promise artifact to default_outputs

def _default_output_impl(ctx: AnalysisContext) -> list[Provider]:
    artifact = ctx.actions.anon_target(_builder, {}).artifact("artifact")
    artifact_with_path = ctx.actions.assert_short_path(artifact, short_path = "dir/hello.out")
    _assert_eq(artifact_with_path.short_path, "dir/hello.out")
    return [DefaultInfo(default_outputs = [artifact])]

_default_output = rule(impl = _default_output_impl, attrs = {})

def _check_default_output_impl(ctx: AnalysisContext) -> list[Provider]:
    _assert_eq(isinstance(ctx.attrs.src, PromiseArtifact), True)
    _assert_eq(ctx.attrs.src.short_path, "dir/hello.out")

    def check_is_artifact(_artifact: Artifact):
        pass

    check_is_artifact(ctx.attrs.src)

    return [DefaultInfo()]

_check_default_output = rule(impl = _check_default_output_impl, attrs = {"src": attrs.source()})

# Test promise artifacts when calling ctx.actions.anon_targets()

def _anon_rule_impl(ctx: AnalysisContext) -> list[Provider]:
    hello = ctx.actions.write("dir/hello.out", ctx.attrs.my_content, has_content_based_path = False)
    return [DefaultInfo(), HelloInfo(output = hello)]

_anon_rule1 = anon_rule(
    impl = _anon_rule_impl,
    attrs = {
        "my_content": attrs.string(default = "content1"),
    },
    artifact_promise_mappings = {
        "artifact": lambda x: x[HelloInfo].output,
    },
)

_anon_rule2 = anon_rule(
    impl = _anon_rule_impl,
    attrs = {
        "my_content": attrs.string(default = "content2"),
    },
    artifact_promise_mappings = {
        "artifact": lambda x: x[HelloInfo].output,
    },
)

def _build_multiple_impl(ctx: AnalysisContext) -> list[Provider]:
    all_targets = ctx.actions.anon_targets([(_anon_rule1, {}), (_anon_rule2, {})])
    artifact1 = all_targets.anon_targets[0].artifact("artifact")
    artifact2 = all_targets.anon_targets[1].artifact("artifact")
    promise = all_targets.promise
    _assert_eq(isinstance(promise, Promise), True)
    out1 = ctx.actions.declare_output("output1", has_content_based_path = False)
    ctx.actions.run(["cp", artifact1, out1.as_output()], category = "cp", identifier = "cp1")
    out2 = ctx.actions.declare_output("output2", has_content_based_path = False)
    ctx.actions.run(["cp", artifact2, out2.as_output()], category = "cp", identifier = "cp2")
    return [DefaultInfo(default_outputs = [out1, out2])]

_build_multiple = rule(impl = _build_multiple_impl, attrs = {})

def _check_multiple_impl(ctx: AnalysisContext) -> list[Provider]:
    out = ctx.actions.declare_output("output", has_content_based_path = False)

    def f(ctx: AnalysisContext, artifacts, outputs):
        _assert_eq(artifacts[ctx.attrs.src[0]].read_string(), "content1")
        _assert_eq(artifacts[ctx.attrs.src[1]].read_string(), "content2")
        ctx.actions.write(outputs[out], "")

    ctx.actions.dynamic_output(dynamic = ctx.attrs.src, inputs = [], outputs = [out.as_output()], f = f)
    return [DefaultInfo(default_output = out)]

_check_multiple = rule(impl = _check_multiple_impl, attrs = {"src": attrs.list(attrs.source())})

def as_artifact_test():
    _build(name = "as_artifact_build")
    _check(name = "as_artifact_check", src = ":as_artifact_build")

    _short_path(name = "as_artifact_short_path")
    _check_symlink_files(name = "symlinked_dir_with_promise_artifact")
    _check_copy(name = "copy_with_promise_artifact")

    _default_output(name = "default_output_with_promise_artifact")
    _check_default_output(name = "default_output_with_promise_artifact_check", src = ":default_output_with_promise_artifact")

    _build_multiple(name = "as_artifact_build_multiple")
    _check_multiple(name = "as_artifact_check_multiple", src = [":as_artifact_build_multiple"])
