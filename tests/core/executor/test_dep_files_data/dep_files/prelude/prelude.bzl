# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

_TEST_NUM_PRECEDING_ACTIONS = read_config("test", "num_preceding_actions", "0")

def _c_binary_impl(ctx):
    headers = {"{}/{}".format(ctx.label.package, h.short_path): h for h in ctx.attrs.headers}

    headers_tag = ctx.actions.artifact_tag()
    use_content_based_paths = ctx.attrs.use_content_based_paths

    headers_dir = ctx.actions.declare_output("headers", has_content_based_path = use_content_based_paths, dir = True)
    headers_dir = ctx.actions.copied_dir(headers_dir, headers)
    headers_dir = headers_tag.tag_artifacts(headers_dir)

    headers_dir_written, _ = ctx.actions.write(
        "headers_dir_written", ctx.attrs.headers_dir_written, has_content_based_path = use_content_based_paths, allow_args = True
    )
    headers_dir_written = headers_tag.tag_artifacts(headers_dir_written)
    headers_dir_written_with_dep_files_placeholder, _ = ctx.actions.write(
        "headers_dir_written_with_dep_files_placeholder",
        ctx.attrs.headers_dir_written,
        use_dep_files_placeholder_for_content_based_paths = True,
        has_content_based_path = use_content_based_paths,
        allow_args = True,
    )

    dep_file = ctx.actions.declare_output("depfile", has_content_based_path = use_content_based_paths)
    app = ctx.actions.declare_output(ctx.attrs.name, has_content_based_path = False)

    cmd = cmd_args(
        [
            ctx.attrs._cc[RunInfo].args,
            ctx.attrs.unused_command_line_param,
            ctx.attrs.main,
            "-I",
            headers_dir,
            "-o",
            app.as_output(),
            "-MMD",
            "-MF",
            headers_tag.tag_artifacts(dep_file.as_output()),
        ],
        hidden = [headers_dir_written, headers_dir_written_with_dep_files_placeholder],
    )

    unused_wrapping_tag = ctx.actions.artifact_tag()
    cmd = unused_wrapping_tag.tag_artifacts(cmd)

    ctx.actions.run(
        cmd,
        category = "cxx_link",
        dep_files = {"headers": headers_tag},
    )

    return [
        DefaultInfo(
            default_output = app,
            sub_targets = {"dep_file": [DefaultInfo(default_output = dep_file)]},
        ),
        RunInfo(args = cmd_args(app)),
    ]

c_binary = rule(
    attrs = {
        "headers": attrs.list(attrs.source()),
        "headers_dir_written": attrs.arg(),
        "main": attrs.source(),
        "unused_command_line_param": attrs.string(),
        "use_content_based_paths": attrs.bool(default = read_config("test", "use_content_based_paths", "true") == "true"),
        "_cc": attrs.dep(default = "root//tools:gcc"),
        "_ignored": attrs.string(default = ""),
    },
    impl = _c_binary_impl,
)

def _tool_impl(ctx):
    return [DefaultInfo(default_output = ctx.attrs.src), RunInfo(args = cmd_args(ctx.attrs.src))]

tool = rule(attrs = {"src": attrs.source()}, impl = _tool_impl)

def _headers_dir_impl(ctx):
    headers = {"{}/{}".format(ctx.label.package, h.short_path): h for h in ctx.attrs.headers}

    headers_dir = ctx.actions.declare_output("headers", has_content_based_path = ctx.attrs.use_content_based_paths, dir = True)
    headers_dir = ctx.actions.copied_dir(headers_dir, headers)

    return [
        DefaultInfo(
            default_output = headers_dir,
        ),
    ]

headers_dir = rule(
    attrs = {
        "headers": attrs.list(attrs.source()),
        "use_content_based_paths": attrs.bool(default = read_config("test", "use_content_based_paths", "true") == "true"),
    },
    impl = _headers_dir_impl,
)

def _simple_dep_file_impl(ctx):
    has_content_based_path = ctx.attrs.use_content_based_paths
    used_input1 = ctx.actions.write("used_input1", ctx.attrs.used_input1_contents, has_content_based_path = has_content_based_path)
    symlink_to_used_input1 = ctx.actions.symlink_file("symlink_to_used_input1", used_input1, has_content_based_path = has_content_based_path)
    symlink_to_symlink_to_used_input1 = ctx.actions.symlink_file(
        "symlink_to_symlink_to_used_input1", symlink_to_used_input1, has_content_based_path = has_content_based_path
    )
    used_input2 = ctx.actions.write("used_input2", ctx.attrs.used_input2_contents, has_content_based_path = has_content_based_path)
    unused_input1 = ctx.actions.write("unused_input1", ctx.attrs.unused_input1_contents, has_content_based_path = has_content_based_path)
    unused_input2 = ctx.actions.write("unused_input2", ctx.attrs.unused_input2_contents, has_content_based_path = has_content_based_path)

    dep_file = ctx.actions.declare_output("depfile", has_content_based_path = has_content_based_path)
    out = ctx.actions.declare_output("out", has_content_based_path = has_content_based_path)

    script = ctx.actions.write(
        "script.py",
        [
            "import sys",
            "with open(sys.argv[1], 'w') as f:",
            "  f.write('output')",
            "with open(sys.argv[2], 'w') as dep_file:",
            "  for arg in sys.argv[3:]:",
            "    dep_file.write('{}\\n'.format(arg))",
        ],
        has_content_based_path = has_content_based_path,
    )

    tag = ctx.actions.artifact_tag()
    args = cmd_args(
        [
            "fbpython",
            script,
            out.as_output(),
            tag.tag_artifacts(dep_file.as_output()),
            tag.tag_artifacts(symlink_to_symlink_to_used_input1),
            tag.tag_artifacts(used_input2),
        ],
        hidden = tag.tag_artifacts(cmd_args([unused_input1, unused_input2])),
    )

    ctx.actions.run(args, category = "test_run", dep_files = {"used": tag})

    return [DefaultInfo(default_output = out)]

simple_dep_file = rule(
    impl = _simple_dep_file_impl,
    attrs = {
        "unused_input1_contents": attrs.string(),
        "unused_input2_contents": attrs.string(),
        "use_content_based_paths": attrs.bool(default = read_config("test", "use_content_based_paths", "true") == "true"),
        "used_input1_contents": attrs.string(),
        "used_input2_contents": attrs.string(),
    },
)

def _identity_projection(value):
    return value

CanonicalJsonInputs = transitive_set(args_projections = {"args": _identity_projection})

def _canonical_json_dep_file_impl(ctx):
    content_paths = ctx.attrs.use_content_based_paths
    used = ctx.actions.write("used", ctx.attrs.used, has_content_based_path = content_paths)
    unused = (
        ctx.actions.symlink_file("unused", ctx.attrs.unused_src, has_content_based_path = content_paths)
        if ctx.attrs.unused_src != None
        else ctx.actions.write("unused", ctx.attrs.unused, has_content_based_path = content_paths)
    )
    untagged = ctx.actions.write("untagged", ctx.attrs.untagged, has_content_based_path = content_paths)
    inputs = [unused, used] if ctx.attrs.reverse else [used, unused]
    if ctx.attrs.repeat:
        inputs.append(inputs[0])
    tag = ctx.actions.artifact_tag()
    inputs = ctx.actions.tset(CanonicalJsonInputs, value = cmd_args(inputs))
    out = ctx.actions.declare_output("out", has_content_based_path = content_paths)
    dep_file = ctx.actions.declare_output("depfile", has_content_based_path = content_paths)
    proto = ctx.actions.declare_output("command.json", has_content_based_path = content_paths)
    fingerprints = []
    input_options = {}
    if ctx.attrs.placement in ["with_inputs", "without_inputs"]:
        input_options["with_inputs"] = ctx.attrs.placement == "with_inputs"
    if ctx.attrs.format == "args":

        def write_args(output, content):
            allow_args = ctx.attrs.placement != "no_allow_args"
            written = ctx.actions.write(
                output,
                content,
                allow_args = allow_args,
                dep_files_fingerprint_using_canonical_paths = True,
                absolute = ctx.attrs.absolute,
                is_executable = ctx.attrs.executable,
                use_dep_files_placeholder_for_content_based_paths = ctx.attrs.placement == "placeholder",
                **input_options,
            )
            if allow_args:
                written, _ = written
            artifact, fingerprint = written
            fingerprints.append(fingerprint)
            return artifact

        content = cmd_args(
            ctx.attrs.option,
            untagged,
            out.as_output(),
            tag.tag_artifacts(dep_file.as_output()),
            tag.tag_artifacts(inputs.project_as_args("args")),
            quote = "shell",
            delimiter = " " if ctx.attrs.pretty else "\n",
        )
        if ctx.attrs.placement == "nested":
            inner = ctx.actions.declare_output("inner.args", has_content_based_path = content_paths)
            inner = write_args(inner, content)
            content = cmd_args(tag.tag_artifacts(inner), format = "@{}")
        proto = write_args(proto, content)
    else:
        proto, fingerprint = ctx.actions.write_json(
            proto,
            {
                "dep_file": tag.tag_artifacts(dep_file.as_output()),
                "inputs": tag.tag_artifacts(inputs.project_as_args("args")),
                "option": ctx.attrs.option,
                "out": out.as_output(),
                "untagged": untagged,
            },
            dep_files_fingerprint_using_canonical_paths = True,
            pretty = ctx.attrs.pretty,
            absolute = ctx.attrs.absolute,
            use_dep_files_placeholder_for_content_based_paths = ctx.attrs.placement == "placeholder",
            **input_options,
        )
        fingerprints.append(fingerprint)
    script = ctx.actions.write(
        "script.py",
        [
            "import json, os, pathlib, shlex, sys",
            "if sys.argv[2] == 'args':",
            "    args = shlex.split(pathlib.Path(sys.argv[1]).read_text())",
            "    if args[0].startswith('@'): args = shlex.split(pathlib.Path(args[0][1:]).read_text())",
            "    option, untagged, out, dep_file, *inputs = args",
            "    command = dict(option=option, untagged=untagged, out=out, dep_file=[dep_file], inputs=inputs)",
            "else:",
            "    command = json.load(open(sys.argv[1]))",
            "used = command['inputs'][0]",
            "output = command['option'] + pathlib.Path(used).read_text() + pathlib.Path(command['untagged']).read_text()",
            "if command['option'] == 'executable-bit': output = str(os.access(sys.argv[1], os.X_OK))",
            "pathlib.Path(command['out']).write_text(output)",
            "pathlib.Path(command['dep_file'][0]).write_text(os.path.relpath(used) + '\\n')",
        ],
        has_content_based_path = content_paths,
    )
    args = cmd_args("fbpython", script)
    if ctx.attrs.placement == "command_line":
        args.add(fingerprints[0])
    else:
        args.add(tag.tag_artifacts(proto))
    if ctx.attrs.placement == "ordinary":
        args.add(cmd_args(hidden = proto))
    args.add(ctx.attrs.format)
    ctx.actions.run(
        args,
        category = "canonical_json_consumer",
        dep_files = {"used": tag},
        dep_file_fingerprints = [proto] if ctx.attrs.placement == "invalid_descriptor" else fingerprints,
        allow_dep_file_cache_upload = True,
    )
    return [DefaultInfo(default_output = out, sub_targets = {"json": [DefaultInfo(default_output = proto)]})]

canonical_json_dep_file = rule(
    impl = _canonical_json_dep_file_impl,
    attrs = {
        "absolute": attrs.bool(default = False),
        "executable": attrs.bool(default = False),
        "format": attrs.string(),
        "option": attrs.string(default = "option"),
        "placement": attrs.string(default = "direct"),
        "pretty": attrs.bool(default = False),
        "repeat": attrs.bool(default = False),
        "reverse": attrs.bool(default = False),
        "untagged": attrs.string(default = "extra"),
        "unused": attrs.string(default = "unused"),
        "unused_src": attrs.option(attrs.source(), default = None),
        "use_content_based_paths": attrs.bool(default = True),
        "used": attrs.string(default = "used"),
    },
)

def _cross_config_run_impl(ctx):
    # A minimal `run` action that copies a marker file to a content-based output.
    marker = ctx.actions.write("marker", ctx.attrs.marker_content, has_content_based_path = True)
    out = ctx.actions.declare_output("out", has_content_based_path = True)
    ctx.actions.run(cmd_args(["cp", marker, out.as_output()]), category = "test_run")
    return [DefaultInfo(default_output = out)]

cross_config_run = rule(
    impl = _cross_config_run_impl,
    attrs = {
        "marker_content": attrs.string(),
        # Unused by the action; only used to force a DICE recompute across builds via a buckconfig change
        "_ignored": attrs.string(default = ""),
    },
)

def _anon_run_impl(ctx):
    out = ctx.actions.declare_output("out", has_content_based_path = ctx.attrs.use_content_based_paths)
    ctx.actions.run(cmd_args(["cp", ctx.attrs.marker, out.as_output()]), category = "anon_run")
    return [DefaultInfo(default_output = out)]

_anon_run = anon_rule(
    impl = _anon_run_impl,
    attrs = {
        "marker": attrs.source(),
        "use_content_based_paths": attrs.bool(),
    },
    artifact_promise_mappings = {"out": lambda p: p[DefaultInfo].default_outputs[0]},
)

def _anon_owned_run_impl(ctx):
    # The marker's path does not depend on its content, so changing `marker_content` changes the
    # anon action's inputs while leaving the anon target's identity alone. This rule declares no
    # run action of its own.
    marker = ctx.actions.write("marker", ctx.attrs.marker_content, has_content_based_path = False)
    out = ctx.actions.anon_target(
        _anon_run,
        {
            "marker": marker,
            "use_content_based_paths": ctx.attrs.use_content_based_paths,
        },
    ).artifact("out")
    if ctx.attrs.use_content_based_paths:
        out = ctx.actions.assert_has_content_based_path(out)
    return [DefaultInfo(default_output = out)]

anon_owned_run = rule(
    impl = _anon_owned_run_impl,
    attrs = {
        "marker_content": attrs.string(),
        "use_content_based_paths": attrs.bool(default = read_config("test", "use_content_based_paths", "true") == "true"),
    },
)

def _shared_dir_dep_file_impl(ctx):
    has_content_based_path = ctx.attrs.use_content_based_paths
    used_input = ctx.actions.write("used_input1", ctx.attrs.used_input_contents, has_content_based_path = has_content_based_path)
    unused_input = ctx.actions.write("unused_input", ctx.attrs.unused_input_contents, has_content_based_path = has_content_based_path)
    dir_inputs = {
        "unused_input": unused_input,
        "used_input": used_input,
    }
    copied_dir = ctx.actions.copied_dir("dir", dir_inputs, has_content_based_path = has_content_based_path)

    symlink_to_used_input_in_dir = ctx.actions.symlink_file(
        "symlink_to_used_input_in_dir", copied_dir.project("used_input"), has_content_based_path = has_content_based_path
    )
    symlink_to_unused_input_in_dir = ctx.actions.symlink_file(
        "symlink_to_unused_input_in_dir", copied_dir.project("unused_input"), has_content_based_path = has_content_based_path
    )

    dep_file = ctx.actions.declare_output("depfile", has_content_based_path = has_content_based_path)
    out = ctx.actions.declare_output("out", has_content_based_path = has_content_based_path)

    script = ctx.actions.write(
        "script.py",
        [
            "import sys",
            "with open(sys.argv[1], 'w') as f:",
            "  f.write('output')",
            "with open(sys.argv[2], 'w') as dep_file:",
            "  for arg in sys.argv[3:]:",
            "    dep_file.write('{}\\n'.format(arg))",
        ],
        has_content_based_path = has_content_based_path,
    )

    tag = ctx.actions.artifact_tag()
    args = cmd_args(
        [
            "fbpython",
            script,
            out.as_output(),
            tag.tag_artifacts(dep_file.as_output()),
            tag.tag_artifacts(symlink_to_used_input_in_dir),
        ],
        hidden = tag.tag_artifacts(cmd_args([symlink_to_unused_input_in_dir])),
    )

    ctx.actions.run(args, category = "test_run", dep_files = {"used": tag})

    return [DefaultInfo(default_output = out)]

shared_dir_dep_file = rule(
    impl = _shared_dir_dep_file_impl,
    attrs = {
        "unused_input_contents": attrs.string(),
        "use_content_based_paths": attrs.bool(default = read_config("test", "use_content_based_paths", "true") == "true"),
        "used_input_contents": attrs.string(),
    },
)

def _dep_file_with_preceding_actions_impl(ctx):
    num_preceding = int(_TEST_NUM_PRECEDING_ACTIONS)

    dummy_script = ctx.actions.write(
        "dummy_script.py",
        [
            "import sys",
            "with open(sys.argv[1], 'w') as f:",
            "  f.write('dummy')",
        ],
        has_content_based_path = False,
    )

    for i in range(num_preceding):
        dummy_out = ctx.actions.declare_output("dummy_{}".format(i))
        ctx.actions.run(
            cmd_args(["fbpython", dummy_script, dummy_out.as_output()]),
            category = "dummy",
            identifier = str(i),
        )

    used_input = ctx.actions.write("used_input", "used_content", has_content_based_path = False)
    unused_input = ctx.actions.write("unused_input", "unused_content", has_content_based_path = False)

    dep_file = ctx.actions.declare_output("depfile")
    out = ctx.actions.declare_output("out")

    script = ctx.actions.write(
        "script.py",
        [
            "import sys",
            "with open(sys.argv[1], 'w') as f:",
            "  f.write('output')",
            "with open(sys.argv[2], 'w') as dep_file:",
            "  for arg in sys.argv[3:]:",
            "    dep_file.write('{}\\n'.format(arg))",
        ],
        has_content_based_path = False,
    )

    tag = ctx.actions.artifact_tag()
    args = cmd_args(
        [
            "fbpython",
            script,
            out.as_output(),
            tag.tag_artifacts(dep_file.as_output()),
            tag.tag_artifacts(used_input),
        ],
        hidden = tag.tag_artifacts(cmd_args([unused_input])),
    )

    ctx.actions.run(args, category = "test_run", dep_files = {"used": tag})

    return [DefaultInfo(default_output = out)]

dep_file_with_preceding_actions = rule(
    impl = _dep_file_with_preceding_actions_impl,
    attrs = {},
)

def _dir_output_dep_file_impl(ctx):
    # A dep-file-producing action whose outputs are a DIRECTORY plus a leaf dep file. Exercises
    # persisting/reloading a directory output (its tree is rehydrated from the materializer).
    has_content_based_path = ctx.attrs.use_content_based_paths
    used_input = ctx.actions.write("used_input", ctx.attrs.used_input_contents, has_content_based_path = has_content_based_path)
    unused_input = ctx.actions.write("unused_input", ctx.attrs.unused_input_contents, has_content_based_path = has_content_based_path)

    dep_file = ctx.actions.declare_output("depfile", has_content_based_path = has_content_based_path)
    out_dir = ctx.actions.declare_output("out_dir", has_content_based_path = has_content_based_path, dir = True)

    script = ctx.actions.write(
        "script.py",
        [
            "import os, sys",
            "out_dir = sys.argv[1]",
            "os.makedirs(out_dir, exist_ok=True)",
            # Echo the used input into the output so a test can tell a freshly produced tree from a
            # stale one reloaded out of the cache.
            "with open(sys.argv[3]) as used_input:",
            "  used_contents = used_input.read()",
            "with open(os.path.join(out_dir, 'f'), 'w') as f:",
            "  f.write(used_contents)",
            "with open(sys.argv[2], 'w') as dep_file:",
            "  for arg in sys.argv[3:]:",
            "    dep_file.write('{}\\n'.format(arg))",
        ],
        has_content_based_path = has_content_based_path,
    )

    tag = ctx.actions.artifact_tag()
    args = cmd_args(
        [
            "fbpython",
            script,
            out_dir.as_output(),
            tag.tag_artifacts(dep_file.as_output()),
            tag.tag_artifacts(used_input),
        ],
        hidden = tag.tag_artifacts(cmd_args([unused_input])),
    )

    ctx.actions.run(args, category = "test_run", dep_files = {"used": tag})

    return [DefaultInfo(default_output = out_dir)]

dir_output_dep_file = rule(
    impl = _dir_output_dep_file_impl,
    attrs = {
        "unused_input_contents": attrs.string(),
        "use_content_based_paths": attrs.bool(default = read_config("test", "use_content_based_paths", "true") == "true"),
        "used_input_contents": attrs.string(),
    },
)

PathArgsInfo = provider(fields = {"args": provider_field(typing.Any)})

def _path_args_impl(ctx):
    out = ctx.actions.write(
        "args",
        [ctx.attrs.src],
        with_inputs = True,
        has_content_based_path = False,
    )
    return [
        DefaultInfo(default_output = out),
        PathArgsInfo(args = cmd_args(out, hidden = ctx.attrs.src)),
    ]

path_args = rule(
    impl = _path_args_impl,
    attrs = {"src": attrs.source()},
)

def _consume_path_args_impl(ctx):
    out = ctx.actions.declare_output("consumed", has_content_based_path = False)
    ctx.actions.run(
        cmd_args([
            "sh",
            "-c",
            'cat "$3" >/dev/null; cp "$(cat "$1")" "$2"',
            "--",
            ctx.attrs.args[PathArgsInfo].args,
            out.as_output(),
            ctx.attrs.trigger,
        ]),
        category = "consume_path_args",
        local_only = True,
    )
    return [DefaultInfo(default_output = out)]

consume_path_args = rule(
    impl = _consume_path_args_impl,
    attrs = {
        "args": attrs.dep(providers = [PathArgsInfo]),
        "trigger": attrs.source(),
    },
)

def _canonical_platforms_impl(ctx):
    return [
        DefaultInfo(),
        ExecutionPlatformRegistrationInfo(
            platforms = [
                ExecutionPlatformInfo(
                    label = ctx.label.raw_target(),
                    configuration = ConfigurationInfo(constraints = {}, values = {}),
                    executor_config = CommandExecutorConfig(
                        local_enabled = True,
                        remote_enabled = True,
                        remote_execution_properties = {"platform": "linux-remote-execution"},
                        remote_execution_use_case = "buck2-testing",
                        allow_cache_uploads = True,
                        remote_dep_file_cache_enabled = True,
                    ),
                )
            ]
        ),
    ]

canonical_platforms = rule(impl = _canonical_platforms_impl, attrs = {})
