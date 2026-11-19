# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

def _create_artifact(ctx: AnalysisContext):
    a = ctx.actions.write("path/test.txt", "", has_content_based_path = False)
    return (a,)

def _create_artifact_declared(ctx: AnalysisContext):
    a = ctx.actions.declare_output("path/test.txt", has_content_based_path = False)
    ctx.actions.write(a, "")
    return (a,)

def _create_artifact_as_output(ctx: AnalysisContext):
    a = ctx.actions.declare_output("path/test.txt", has_content_based_path = False)
    ctx.actions.write(a, "")
    return (a.as_output(),)

def _check_artifact(x):
    # Want an array
    [x] = x
    x = x.replace("\\", "/")
    if not x.startswith("buck-out/") or not x.endswith("path/test.txt"):
        fail("Output is not as expected, got " + repr(x))

def _create_cmdargs_artifact(ctx: AnalysisContext):
    a = ctx.actions.write("magic/path/test.txt", "", has_content_based_path = False)
    return cmd_args(["a", cmd_args(a, parent = 1)])

def _check_cmdargs_artifact(x):
    [a, b] = x
    b = b.replace("\\", "/")
    if a != "a" or not b.startswith("buck-out/") or not b.endswith("magic/path"):
        fail("Output is not as expected, got " + repr(x))

def _create_target(ctx: AnalysisContext):
    # We don't want to hardcode the label, as different roots may change it,
    # so instead check it matches the string representation
    return [ctx.label.raw_target(), str(ctx.label.raw_target())]

def _check_target(x):
    [a, b] = x
    if a != b:
        fail("Targets should match, got " + repr(x))

def _create_label(ctx: AnalysisContext):
    # We don't want to hardcode the label, as different roots may change it,
    # so instead check it matches the string representation
    return [ctx.label, str(ctx.label)]

def _check_label(x):
    [a, b] = x
    if a != b:
        fail("Labels should match, got " + repr(x))

def _create_enum_value(_ctx: AnalysisContext):
    typ = enum("foo")
    return [typ("foo"), "foo"]

def _check_enum_value(x):
    [a, b] = x
    if a != b:
        fail("Enum values should match, got " + repr(x))

TestProvider = provider(fields = ["foo"])

def _create_provider_value(ctx: AnalysisContext):
    a = ctx.actions.write("path/test.txt", "", has_content_based_path = False)
    return [TestProvider(foo = a), a]

def _check_provider_value(x):
    [prov, a] = x
    if prov["foo"] != a:
        fail("Provider values should match, got " + repr(x))

_MyRec = record(hello = typing.Any, bye = typing.Any)

tests = [
    ("atom", lambda _: "test", "test"),
    ("simple", lambda _: [1], [1]),
    ("nested", lambda _: [42, {"test": True}], [42, {"test": True}]),
    ("record", lambda _: _MyRec(hello = [1], bye = {}), {"bye": {}, "hello": [1]}),
    ("struct", lambda _: struct(hello = [1], bye = struct()), {"bye": {}, "hello": [1]}),
    ("artifact", _create_artifact, _check_artifact),
    ("artifact_declared", _create_artifact_declared, _check_artifact),
    ("artifact_output", _create_artifact_as_output, _check_artifact),
    ("target", _create_target, _check_target),
    ("label", _create_label, _check_label),
    ("cmdargs", lambda _: {"more": cmd_args(["a", "b", "c"], format = "1{}")}, {"more": ["1a", "1b", "1c"]}),
    ("cmdargs_single", lambda _: {"test": cmd_args("abc")}, {"test": ["abc"]}),
    ("cmdargs_concat", lambda _: {"test": cmd_args("abc", delimiter = "")}, {"test": "abc"}),
    ("cmdargs_artifact", _create_cmdargs_artifact, _check_cmdargs_artifact),
    ("enum", _create_enum_value, _check_enum_value),
    ("provider", _create_provider_value, _check_provider_value),
]

def _write_json_rule_impl(ctx: AnalysisContext) -> list[Provider]:
    want = ctx.label.name
    for name, input, output in tests:
        if name == want:
            input_file = ctx.actions.write_json("input", input(ctx), has_content_based_path = False)
            output_file = ctx.actions.declare_output("output", has_content_based_path = False)

            def f(ctx: AnalysisContext, artifacts, outputs):
                contents = artifacts[input_file].read_json()
                if type(output) == "function":
                    output(contents)
                elif contents == output:
                    pass
                else:
                    fail("JSON divergence in " + name + ": Got " + repr(contents) + ", wanted " + repr(output))
                ctx.actions.write(outputs[output_file], "")

            ctx.actions.dynamic_output(dynamic = [input_file], inputs = [], outputs = [output_file.as_output()], f = f)
            return [DefaultInfo(default_output = output_file)]
    fail("Test named " + want + " not found")

write_json_rule = rule(impl = _write_json_rule_impl, attrs = {})

def _write_json_pretty_rule_impl(ctx: AnalysisContext) -> list[Provider]:
    value = {"key1": [1], "key2": [True, False]}

    # @unsorted-dict-items
    tests = {
        "default": (
            ctx.actions.write_json("default_input", value, has_content_based_path = False),
            ctx.actions.declare_output("default_output", has_content_based_path = False),
            '{"key1":[1],"key2":[true,false]}',
        ),
        "compact": (
            ctx.actions.write_json("compact_input", value, pretty = False, has_content_based_path = False),
            ctx.actions.declare_output("compact_output", has_content_based_path = False),
            '{"key1":[1],"key2":[true,false]}',
        ),
        "pretty": (
            ctx.actions.write_json("pretty_input", value, pretty = True, has_content_based_path = False),
            ctx.actions.declare_output("pretty_output", has_content_based_path = False),
            '{\n  "key1": [\n    1\n  ],\n  "key2": [\n    true,\n    false\n  ]\n}\n',
        ),
    }

    if tests["default"][2] != tests["compact"][2]:
        fail("The default for no 'pretty' must be compact")

    def f(ctx: AnalysisContext, artifacts, outputs):
        for k, (input, output, expected) in tests.items():
            actual = artifacts[input].read_string()
            if actual != expected:
                fail("JSON divergence in '{}': Got {}, wanted {}", k, repr(actual), repr(expected))
            ctx.actions.write(outputs[output], "")

    ctx.actions.dynamic_output(
        dynamic = [input for input, _, _ in tests.values()],
        inputs = [],
        outputs = [output.as_output() for _, output, _ in tests.values()],
        f = f,
    )

    return [DefaultInfo(default_outputs = [output for _, output, _ in tests.values()])]

write_json_pretty_rule = rule(impl = _write_json_pretty_rule_impl, attrs = {})

def _write_json_with_inputs_rule(ctx: AnalysisContext) -> list[Provider]:
    input = ctx.actions.write("input", ctx.attrs.content, has_content_based_path = False)
    as_json = ctx.actions.write_json("json", input, with_inputs = True, has_content_based_path = False)

    output = ctx.actions.declare_output("output", has_content_based_path = False)

    # as_json will contain a quoted-path and we want to read the contents of that path
    script = ctx.actions.write(
        "script.py",
        ["import sys;p_fp=open(sys.argv[1],'r');p=p_fp.read().replace('\"',\"\");i_fp=open(p,'r');i=i_fp.read();o_fp=open(sys.argv[2],'w');o_fp.write(i)"],
        has_content_based_path = False,
    )
    cmd = cmd_args("fbpython", script, as_json, output.as_output())
    ctx.actions.run(cmd, category = "cmd")

    marker = ctx.actions.declare_output("marker", has_content_based_path = False)

    def f(ctx: AnalysisContext, artifacts, outputs):
        expected = artifacts[input].read_string()
        actual = artifacts[output].read_string()
        if expected != actual:
            fail("mismatched output. expected `{}`, actual `{}`".format(expected, actual))
        ctx.actions.write(outputs[marker], "")

    ctx.actions.dynamic_output(dynamic = [input, output], inputs = [], outputs = [marker.as_output()], f = f)
    return [DefaultInfo(default_output = marker)]

write_json_with_inputs_rule = rule(impl = _write_json_with_inputs_rule, attrs = {"content": attrs.string()})

def _write_json_absolute_rule(ctx: AnalysisContext) -> list[Provider]:
    src = ctx.attrs.dep[DefaultInfo].default_outputs[0]

    out = ctx.actions.write_json(
        "out.json",
        [src, cmd_args(src, delimiter = "")],
        absolute = True,
        has_content_based_path = False,
    )

    return [DefaultInfo(out)]

write_json_absolute_rule = rule(impl = _write_json_absolute_rule, attrs = {"dep": attrs.dep()})

def test():
    for name, _, _ in tests:
        write_json_rule(name = name)

    write_json_pretty_rule(
        name = "pretty",
    )

    inputs_content = read_config("write_json", "content")
    if inputs_content == None:
        fail("config value write_json.content required")
    write_json_with_inputs_rule(
        name = "with_inputs",
        content = inputs_content,
    )
    write_json_absolute_rule(name = "absolute", dep = ":atom")
