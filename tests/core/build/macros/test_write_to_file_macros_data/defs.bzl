# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

_TEST_DEP_FILES_FINGERPRINT_USING_CANONICAL_PATHS = read_config("test", "dep_files_fingerprint_using_canonical_paths", "false")
_TEST_PYTHON = read_config("test", "python")

def _write_file(ctx):
    f = ctx.actions.write("write_file.txt", "test test test", has_content_based_path = False)
    return [DefaultInfo(default_output = f)]

write_file = rule(
    impl = _write_file,
    attrs = {},
)

def _test_rule(ctx):
    arg = ctx.attrs.arg

    f = ctx.actions.declare_output("out.txt", has_content_based_path = False)
    fingerprint = _TEST_DEP_FILES_FINGERPRINT_USING_CANONICAL_PATHS == "true"
    written, _ = ctx.actions.write(
        f,
        [
            cmd_args(arg, hidden = arg),
            cmd_args(arg, relative_to = f),
            cmd_args(cmd_args(arg, relative_to = f)),
            cmd_args(cmd_args(arg), relative_to = f),
        ],
        allow_args = True,
        with_inputs = not fingerprint,
        dep_files_fingerprint_using_canonical_paths = fingerprint,
    )

    if fingerprint:
        artifact, descriptor = written
        copied = ctx.actions.declare_output("copy.txt")
        ctx.actions.run(
            [_TEST_PYTHON, "-c", "import shutil,sys; shutil.copyfile(sys.argv[1],sys.argv[2])", artifact, copied.as_output()],
            category = "copy_args",
            dep_file_fingerprints = [descriptor],
            local_only = True,
        )
        return [DefaultInfo(default_output = f, other_outputs = [copied])]
    return [DefaultInfo(default_output = written)]

test_rule = rule(
    impl = _test_rule,
    attrs = {
        "arg": attrs.arg(),
    },
)

def _input(ctx):
    if ctx.attrs.fail:
        output = ctx.actions.declare_output("input.txt", has_content_based_path = True)
        ctx.actions.run(
            cmd_args("fbpython", "-c", "raise RuntimeError('Unrelated input requested')", hidden = output.as_output()),
            category = "fail_input",
            local_only = True,
        )
    else:
        output = ctx.actions.write("input.txt", "payload", has_content_based_path = True)
    return [DefaultInfo(default_output = output)]

input = rule(
    impl = _input,
    attrs = {"fail": attrs.bool(default = False)},
)

def _macro_writer(ctx):
    content = cmd_args(ctx.attrs.inline)
    if ctx.attrs.hidden:
        content.add(cmd_args(hidden = ctx.attrs.macro))
    else:
        content.add(ctx.attrs.macro)
    if ctx.attrs.tagged:
        content = ctx.actions.artifact_tag().tag_artifacts(content)
    script, macros = ctx.actions.write("script", content, allow_args = True)
    return [DefaultInfo(default_output = script, sub_targets = {"macros": [DefaultInfo(default_outputs = macros)]})]

macro_writer = rule(
    impl = _macro_writer,
    attrs = {
        "hidden": attrs.bool(default = False),
        "inline": attrs.arg(default = ""),
        "macro": attrs.arg(),
        "tagged": attrs.bool(default = False),
    },
)

def _tagged_writer(ctx):
    content = cmd_args("payload", hidden = ctx.attrs.dep[DefaultInfo].default_outputs)
    tag = ctx.actions.artifact_tag()
    content = tag.tag_inputs(content) if ctx.attrs.inputs_only else tag.tag_artifacts(content)
    script = ctx.actions.write("script", content, with_inputs = ctx.attrs.with_inputs)
    json_output = ctx.actions.declare_output("output.json")
    json = ctx.actions.write_json(json_output, content, with_inputs = ctx.attrs.with_inputs)
    run = ctx.actions.declare_output("run")
    ctx.actions.run(
        cmd_args("fbpython", "-c", "import pathlib, sys; pathlib.Path(sys.argv[1]).touch()", run.as_output(), hidden = content),
        category = "run",
        local_only = True,
    )
    return [
        RunInfo(args = content),
        DefaultInfo(
            default_output = script,
            sub_targets = {
                "json": [DefaultInfo(default_output = json_output, other_outputs = [json])],
                "run": [DefaultInfo(default_output = run)],
                "script": [DefaultInfo(default_output = script)],
            },
        ),
    ]

tagged_writer = rule(
    impl = _tagged_writer,
    attrs = {
        "dep": attrs.dep(),
        "inputs_only": attrs.bool(default = False),
        "with_inputs": attrs.bool(default = False),
    },
)
