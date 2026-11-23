# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

# Regression fixture for a writer-vs-writer race on shared content-based
# paths, minimized from prelude/rust/build.bzl
# _create_transitive_dependency_symlinks.
#
# When a target that uses has_content_based_path = True for a write_json
# output is analyzed under many configurations, every analysis independently
# submits its own declare_write. Each call runs cleanup_path -> write_file
# via the immediate-write path, and the cleanup operations race on the
# shared __<target>__/<content_hash>/ on-disk path. The losing writer's
# remove then returns ENOENT, which caused the OSS bootstrap
# "No such file or directory" failures that motivated D101857169.
#
# The fixture defines a single shared producer target and a consumer rule
# that exercises the output; TARGETS.fixture instantiates many consumers
# under different platforms so the producer is re-analyzed once per
# configuration.

_CBP_RACE_SEED = read_config("cbp_race", "seed", "0")
_TEST_URL = read_config("test", "url")
_TEST_SHA1 = read_config("test", "sha1")
_TEST_SHA512 = read_config("test", "sha512")

def _rust_pattern_producer_impl(ctx):
    # Stable content that does NOT depend on configuration. A per-test-run
    # seed (via buckconfig) invalidates the RE cache between runs, but
    # every configuration within one run produces the same JSON, so every
    # analysis resolves to the same content-hash path.
    out = ctx.actions.write_json(
        ctx.actions.declare_output("artifacts.json", has_content_based_path = True),
        {"seed": _CBP_RACE_SEED},
        pretty = True,
    )
    return [DefaultInfo(default_output = out)]

rust_pattern_producer = rule(
    impl = _rust_pattern_producer_impl,
    attrs = {},
)

def _rust_pattern_consumer_impl(ctx):
    out = ctx.actions.declare_output("out.txt")
    producer_out = ctx.attrs.producer[DefaultInfo].default_outputs[0]

    # Mirror the rust `deps` action: cmd_args with format="--artifacts={}"
    # so the consumer references the producer's on-disk content-based path.
    ctx.actions.run(
        cmd_args(
            "fbpython",
            "-c",
            "import argparse\np = argparse.ArgumentParser()\np.add_argument('--artifacts', type=argparse.FileType('r'), required=True)\np.add_argument('--out', required=True)\na = p.parse_args()\nopen(a.out, 'w').write(a.artifacts.read())",
            cmd_args(producer_out, format = "--artifacts={}"),
            cmd_args(out.as_output(), format = "--out={}"),
        ),
        category = "consume_cbp",
        local_only = True,
    )
    return [DefaultInfo(default_output = out)]

rust_pattern_consumer = rule(
    impl = _rust_pattern_consumer_impl,
    attrs = {
        "producer": attrs.dep(),
    },
)

def project(f: Artifact):
    return f

NameSet = transitive_set(
    args_projections = {
        "project": project,
    }
)

def _write_with_content_based_path_impl(ctx):
    artifact_input = ctx.actions.write("artifact_input", "artifact_input", has_content_based_path = True)

    tset_item1 = ctx.actions.declare_output("tset_item1", has_content_based_path = True)
    tset1 = ctx.actions.tset(NameSet, value = ctx.actions.write(tset_item1, "tset_item1"))
    tset_item2 = ctx.actions.declare_output("tset_item2", has_content_based_path = True)
    tset2 = ctx.actions.tset(NameSet, value = ctx.actions.write(tset_item2, "tset_item2"))
    tset = ctx.actions.tset(NameSet, children = [tset1, tset2])

    out = ctx.actions.declare_output("out.txt", has_content_based_path = True)
    return [DefaultInfo(default_output = ctx.actions.write(out, cmd_args(artifact_input, ctx.attrs.data, tset.project_as_args("project"))))]

write_with_content_based_path = rule(
    impl = _write_with_content_based_path_impl,
    attrs = {
        "data": attrs.string(),
    },
)

def _write_macro_with_content_based_path_impl(ctx):
    out = ctx.actions.declare_output("out.txt", has_content_based_path = True)
    out, macro_files = ctx.actions.write(out, ctx.attrs.arg, allow_args = True)
    if not len(macro_files) == 1:
        fail("Expected exactly one macro file, got {}".format(macro_files))
    else:
        macro_file = macro_files[0]

    return [DefaultInfo(default_output = macro_file, other_outputs = [out])]

write_macro_with_content_based_path = rule(
    impl = _write_macro_with_content_based_path_impl,
    attrs = {
        "arg": attrs.arg(),
    },
)

def _write_json_with_content_based_path_impl(ctx):
    out = ctx.actions.write_json(
        "out.json",
        {
            "baz": 42,
            "foo": "bar",
            "source": ctx.attrs.source,
        },
        has_content_based_path = True,
    )
    return [DefaultInfo(default_output = out)]

write_json_with_content_based_path = rule(
    impl = _write_json_with_content_based_path_impl,
    attrs = {
        "source": attrs.source(),
    },
)

def _run_with_content_based_path_impl(ctx):
    script = ctx.actions.declare_output("script.py", has_content_based_path = True)
    script = ctx.actions.write(
        script,
        [
            "import sys",
            "with open(sys.argv[1], 'w') as f:",
            "  f.write(sys.argv[2])",
        ],
    )

    out = ctx.actions.declare_output("out", has_content_based_path = True)
    args = cmd_args(["fbpython", script, out.as_output(), ctx.attrs.data])
    args.add(cmd_args(hidden = ctx.attrs.depends_on))
    kwargs = {
        "category": "test_run",
        "expect_eligible_for_dedupe": True,
        "outputs_for_error_handler": [out.as_output()],
    }
    if ctx.attrs.prefer_local:
        kwargs["prefer_local"] = True
    else:
        kwargs["prefer_remote"] = True

    ctx.actions.run(args, **kwargs)

    return [DefaultInfo(default_output = out), RunInfo(args = [out])]

run_with_content_based_path = rule(
    impl = _run_with_content_based_path_impl,
    attrs = {
        "data": attrs.string(),
        "depends_on": attrs.list(attrs.source(), default = []),
        "prefer_local": attrs.bool(default = False),
        "_ignored": attrs.string(default = "ignored"),
    },
)

def _copy_impl(ctx):
    out = ctx.actions.copy_file("out", ctx.attrs.to_copy, has_content_based_path = True)

    return [DefaultInfo(default_output = out)]

copy = rule(
    impl = _copy_impl,
    attrs = {
        "to_copy": attrs.source(),
    },
)

def _symlink_impl(ctx):
    out = ctx.actions.symlink_file("out", ctx.attrs.to_symlink, has_content_based_path = True)

    return [DefaultInfo(default_output = out)]

symlink = rule(
    impl = _symlink_impl,
    attrs = {
        "to_symlink": attrs.source(),
    },
)

def _symlink_and_copy_impl(ctx):
    written = ctx.actions.write("written", "written", has_content_based_path = True)
    symlink = ctx.actions.symlink_file("symlink", written, has_content_based_path = True)

    script = ctx.actions.declare_output("script.py", has_content_based_path = True)
    script = ctx.actions.write(
        script,
        [
            "import shutil",
            "import sys",
            "shutil.copyfile(sys.argv[1], sys.argv[2])",
        ],
    )

    out = ctx.actions.declare_output("out", has_content_based_path = True)
    args = cmd_args(["fbpython", script, symlink, out.as_output()])

    ctx.actions.run(args, category = "test_run")

    return [DefaultInfo(default_output = out)]

symlink_and_copy = rule(
    impl = _symlink_and_copy_impl,
    attrs = {},
)

def _copied_dir_impl(ctx):
    another_to_copy = ctx.actions.declare_output("another_to_copy", has_content_based_path = True)
    ctx.actions.write(another_to_copy, "another_to_copy")
    out = ctx.actions.copied_dir(
        "out",
        {
            "another_to_copy": another_to_copy,
            "to_copy": ctx.attrs.to_copy,
        },
        has_content_based_path = True,
    )

    return [DefaultInfo(default_output = out)]

copied_dir = rule(
    impl = _copied_dir_impl,
    attrs = {
        "to_copy": attrs.source(),
    },
)

def _symlinked_dir_impl(ctx):
    another_to_symlink = ctx.actions.declare_output("another_to_symlink", has_content_based_path = True)
    ctx.actions.write(another_to_symlink, "another_to_symlink")
    out = ctx.actions.symlinked_dir(
        "out",
        {
            "another_to_symlink": another_to_symlink,
            "to_symlink": ctx.attrs.to_symlink,
        },
        has_content_based_path = True,
    )

    return [DefaultInfo(default_output = out)]

symlinked_dir = rule(
    impl = _symlinked_dir_impl,
    attrs = {
        "to_symlink": attrs.source(),
    },
)

def _assembled_dir_impl(ctx):
    to_copy = ctx.actions.declare_output("to_copy", has_content_based_path = True)
    ctx.actions.write(to_copy, "to_copy")
    to_symlink = ctx.actions.declare_output("to_symlink", has_content_based_path = True)
    ctx.actions.write(to_symlink, "to_symlink")
    out = ctx.actions.assembled_dir(
        "out",
        contents = {
            "copied": assembled_dir.copy(to_copy),
            "copied_dep": assembled_dir.copy(ctx.attrs.dep),
            "symlinked": assembled_dir.symlink(to_symlink),
            "symlinked_dep": assembled_dir.symlink(ctx.attrs.dep),
        },
        has_content_based_path = True,
    )

    return [DefaultInfo(default_output = out)]

assembled_dir_rule = rule(
    impl = _assembled_dir_impl,
    attrs = {
        "dep": attrs.source(),
    },
)

def _cas_artifact_with_content_based_path_impl(ctx: AnalysisContext):
    out = ctx.actions.cas_artifact(
        "out",
        ctx.attrs.digest,
        ctx.attrs.use_case,
        expires_after_timestamp = ctx.attrs.expires_after_timestamp,
        is_tree = ctx.attrs.is_tree,
        is_directory = ctx.attrs.is_directory,
        has_content_based_path = True,
    )
    return [DefaultInfo(default_output = out)]

cas_artifact_with_content_based_path = rule(
    impl = _cas_artifact_with_content_based_path_impl,
    attrs = {
        "digest": attrs.string(),
        "expires_after_timestamp": attrs.int(),
        "is_directory": attrs.bool(default = False),
        "is_tree": attrs.bool(default = False),
        "use_case": attrs.string(),
    },
)

def _download_with_content_based_path_impl(ctx: AnalysisContext):
    url = _TEST_URL

    if ctx.attrs.defer_download:
        checksums = {"sha1": _TEST_SHA1}
    else:
        # The CAS doesn't use sha512 digests, so the action can't defer the download. It hashes
        # the downloaded file to find the output path instead.
        checksums = {"sha512": _TEST_SHA512}

    download = ctx.actions.download_file("download", url, has_content_based_path = True, **checksums)
    return [
        DefaultInfo(default_output = download),
    ]

download_with_content_based_path = rule(
    impl = _download_with_content_based_path_impl,
    attrs = {
        "defer_download": attrs.bool(default = True),
    },
)

def _failing_validation_with_content_based_path_impl(ctx):
    validation = ctx.actions.declare_output("validation.json", has_content_based_path = True)
    validation = ctx.actions.write_json(
        validation,
        {
            "data": {
                "message": "This is a failing validation",
                "status": "failure",
            },
            "version": 1,
        },
        pretty = True,
    )

    return [
        DefaultInfo(default_output = ctx.actions.write("out", "hello world", has_content_based_path = False)),
        ValidationInfo(
            validations = [
                ValidationSpec(
                    name = "whistle",
                    validation_result = validation,
                ),
            ]
        ),
    ]

failing_validation_with_content_based_path = rule(
    impl = _failing_validation_with_content_based_path_impl,
    attrs = {},
)

def _dynamic_with_content_based_path_impl(ctx: AnalysisContext) -> list[Provider]:
    input = ctx.actions.declare_output("input", has_content_based_path = True)
    input = ctx.actions.write(input, str("input"))
    output = ctx.actions.declare_output("out", has_content_based_path = True)

    def f(ctx: AnalysisContext, artifacts, outputs):
        src = artifacts[input].read_string()
        ctx.actions.write(outputs[output], src)

    ctx.actions.dynamic_output(dynamic = [input], inputs = [], outputs = [output.as_output()], f = f)
    return [DefaultInfo(default_output = output)]

dynamic_with_content_based_path = rule(
    impl = _dynamic_with_content_based_path_impl,
    attrs = {},
)

def _basic_dynamic_output_new_impl(actions: AnalysisActions, src: ArtifactValue, out: OutputArtifact):
    actions.write(out, src.read_string())
    return []

_basic_dynamic_output_new = dynamic_actions(
    impl = _basic_dynamic_output_new_impl,
    attrs = {
        "out": dynattrs.output(),
        "src": dynattrs.artifact_value(),
    },
)

def _dynamic_new_with_content_based_path_impl(ctx: AnalysisContext) -> list[Provider]:
    input = ctx.actions.declare_output("input", has_content_based_path = True)
    input = ctx.actions.write(input, str("input"))
    output = ctx.actions.declare_output("out", has_content_based_path = True)

    ctx.actions.dynamic_output_new(
        _basic_dynamic_output_new(
            src = input,
            out = output.as_output(),
        )
    )
    return [DefaultInfo(default_output = output)]

dynamic_new_with_content_based_path = rule(
    impl = _dynamic_new_with_content_based_path_impl,
    attrs = {},
)

def _use_projection_with_content_based_path_impl(ctx):
    script = ctx.actions.declare_output("script.py", has_content_based_path = True)
    script = ctx.actions.write(
        script,
        [
            "import os",
            "import sys",
            "os.makedirs(sys.argv[1], exist_ok=True)",
            "with open(sys.argv[2], 'w') as f:",
            "  f.write('hello projection1')",
            "with open(sys.argv[3], 'w') as f:",
            "  f.write('hello projection2')",
        ],
    )

    out = ctx.actions.declare_output("out", has_content_based_path = True)
    projection1 = out.project("projection1.txt")
    projection2 = out.project("projection2.txt")
    args = cmd_args(["fbpython", script, out.as_output(), projection1.as_output(), projection2.as_output()])
    ctx.actions.run(args, category = "test_run", prefer_remote = True)

    copy_script = ctx.actions.write(
        "copy_script.py",
        [
            "import shutil",
            "import sys",
            "shutil.copyfile(sys.argv[1], sys.argv[2])",
        ],
        has_content_based_path = True,
    )

    first_copy_projection1 = ctx.actions.declare_output("first_copied_projection1.txt", has_content_based_path = True)
    args = cmd_args(["fbpython", copy_script, projection1, first_copy_projection1.as_output()], hidden = [out])
    ctx.actions.run(args, category = "test_first_copy_projection1", prefer_local = True)

    second_copy_projection1 = ctx.actions.declare_output("second_copied_projection1.txt", has_content_based_path = True)
    args = cmd_args(["fbpython", copy_script, projection1, second_copy_projection1.as_output()], hidden = [first_copy_projection1])
    ctx.actions.run(args, category = "test_second_copy_projection1", prefer_local = True)

    return [DefaultInfo(default_output = second_copy_projection1)]

use_projection_with_content_based_path = rule(
    impl = _use_projection_with_content_based_path_impl,
    attrs = {},
)

def _slow_running_local_action_with_content_based_path_impl(ctx):
    script = ctx.actions.declare_output("script.py", has_content_based_path = True)
    script = ctx.actions.write(
        script,
        [
            "import sys",
            "import time",
            "with open(sys.argv[1], 'w') as f:",
            "  f.write(sys.argv[2])",
            "time.sleep(2)",
        ],
    )

    out = ctx.actions.declare_output("out", has_content_based_path = True)
    args = cmd_args(["fbpython", script, out.as_output(), ctx.attrs.data])

    ctx.actions.run(args, category = "test_run", local_only = True)

    return [DefaultInfo(default_output = out)]

slow_running_local_action_with_content_based_path = rule(
    impl = _slow_running_local_action_with_content_based_path_impl,
    attrs = {
        "data": attrs.string(),
    },
)

def _writes_input_to_output_impl(ctx):
    script = ctx.actions.declare_output("script.py", has_content_based_path = True)
    script = ctx.actions.write(
        script,
        [
            "import sys",
            "with open(sys.argv[1], 'w') as output:",
            "  with open(sys.argv[2], 'r') as input:",
            "    output.write(input.read())",
        ],
    )

    out = ctx.actions.declare_output("out", has_content_based_path = True)
    args = cmd_args(["fbpython", script, out.as_output(), ctx.attrs.input])

    ctx.actions.run(args, category = "test_run", local_only = True)

    return [DefaultInfo(default_output = out)]

writes_input_to_output = rule(
    impl = _writes_input_to_output_impl,
    attrs = {
        "input": attrs.source(),
    },
)

def _uses_relative_to_impl(ctx):
    out1 = ctx.actions.declare_output("out1", has_content_based_path = True)
    ctx.actions.write(out1, "hello world")

    out2 = ctx.actions.declare_output("out2", has_content_based_path = True)
    ctx.actions.write(out2, "hello world")

    out3 = ctx.actions.declare_output("out3", has_content_based_path = True)
    ctx.actions.write(out3, cmd_args(out1, relative_to = out2))

    return [DefaultInfo(default_output = out3)]

uses_relative_to = rule(
    impl = _uses_relative_to_impl,
    attrs = {},
)

def _sets_inconsistent_params_impl(ctx):
    out = ctx.actions.declare_output("out", has_content_based_path = True)
    ctx.actions.write(out, "hello world", has_content_based_path = False)

    return [DefaultInfo(default_output = out)]

sets_inconsistent_params = rule(
    impl = _sets_inconsistent_params_impl,
    attrs = {},
)

def _argsfile_with_incorrectly_declared_output_impl(ctx):
    script = ctx.actions.declare_output("script.py", has_content_based_path = True)
    out = ctx.actions.declare_output("out", has_content_based_path = True)
    script = ctx.actions.write(
        script,
        [
            "import sys",
            cmd_args(out, format = "with open('{}', 'w') as f:"),
            "  f.write('blah')",
        ],
    )

    args = cmd_args(["fbpython", script], hidden = [out.as_output()])

    ctx.actions.run(args, category = "test_run")

    return [DefaultInfo(default_output = out), RunInfo(args = [out])]

argsfile_with_incorrectly_declared_output = rule(
    impl = _argsfile_with_incorrectly_declared_output_impl,
    attrs = {},
)

def _incremental_action_impl(ctx) -> list[Provider]:
    script = ctx.actions.write(
        "script.py",
        [
            "import sys",
            "with open(sys.argv[1], 'w') as f:",
            "  f.write('hello')",
        ],
        has_content_based_path = True,
    )

    out = ctx.actions.declare_output("out", has_content_based_path = True)
    args = cmd_args(["fbpython", script, out.as_output()])

    ctx.actions.run(
        args,
        category = "test_incremental_run",
        no_outputs_cleanup = True,
        metadata_env_var = "METADATA",
        metadata_path = "metadata.json",
    )

    return [DefaultInfo(out)]

incremental_action = rule(impl = _incremental_action_impl, attrs = {})

HelloInfo = provider(fields = ["hello"])

def _anon_impl(ctx: AnalysisContext) -> list[Provider]:
    hello = ctx.actions.write("hello.out", "hello", has_content_based_path = ctx.attrs.has_content_based_path)
    return [DefaultInfo(), HelloInfo(hello = hello)]

_anon = anon_rule(
    impl = _anon_impl,
    attrs = {
        "has_content_based_path": attrs.bool(),
    },
    artifact_promise_mappings = {
        "hello": lambda x: x[HelloInfo].hello,
    },
)

def _resolve_promise_artifact_impl(ctx: AnalysisContext) -> list[Provider]:
    anon = ctx.actions.anon_target(_anon, {"has_content_based_path": ctx.attrs.artifact_has_content_based_path})
    hello_artifact = anon.artifact("hello")

    if ctx.attrs.assert_promised_artifact_has_content_based_path:
        hello_artifact = ctx.actions.assert_has_content_based_path(hello_artifact)

    written = ctx.actions.write("hello.out", hello_artifact, has_content_based_path = False)

    return [DefaultInfo(default_output = written)]

resolve_promise_artifact = rule(
    impl = _resolve_promise_artifact_impl,
    attrs = {
        "artifact_has_content_based_path": attrs.bool(),
        "assert_promised_artifact_has_content_based_path": attrs.bool(),
    },
)

AnonConsumerInfo = provider(fields = ["out"])

def _anon_consumer_impl(ctx: AnalysisContext) -> list[Provider]:
    out = ctx.actions.write("consumer_out", ctx.attrs.input, has_content_based_path = False)
    return [DefaultInfo(), AnonConsumerInfo(out = out)]

_anon_consumer = anon_rule(
    impl = _anon_consumer_impl,
    attrs = {
        "input": attrs.source(),
    },
    artifact_promise_mappings = {
        "out": lambda x: x[AnonConsumerInfo].out,
    },
)

def _pass_cbp_promise_to_anon_target_impl(ctx: AnalysisContext) -> list[Provider]:
    producer = ctx.actions.anon_target(_anon, {"has_content_based_path": True})
    hello_artifact = producer.artifact("hello")
    hello_artifact = ctx.actions.assert_has_content_based_path(hello_artifact)

    consumer = ctx.actions.anon_target(_anon_consumer, {"input": hello_artifact})
    consumer_out = consumer.artifact("out")

    return [DefaultInfo(default_output = consumer_out)]

pass_cbp_promise_to_anon_target = rule(
    impl = _pass_cbp_promise_to_anon_target_impl,
    attrs = {},
)

AnonNonCbpInfo = provider(fields = ["artifact"])

def _anon_non_cbp_impl(ctx: AnalysisContext) -> list[Provider]:
    out = ctx.actions.write("out", "anon_content", has_content_based_path = False)
    return [DefaultInfo(), AnonNonCbpInfo(artifact = out)]

_anon_non_cbp = anon_rule(
    impl = _anon_non_cbp_impl,
    attrs = {},
    artifact_promise_mappings = {
        "out": lambda x: x[AnonNonCbpInfo].artifact,
    },
)

def _anon_non_cbp_wrapper_impl(ctx):
    anon = ctx.actions.anon_target(_anon_non_cbp, {})
    anon_artifact = anon.artifact("out")
    return [DefaultInfo(), AnonNonCbpInfo(artifact = anon_artifact)]

anon_non_cbp_wrapper = rule(
    impl = _anon_non_cbp_wrapper_impl,
    attrs = {},
)

def _run_with_anon_non_cbp_dep_impl(ctx):
    anon_artifact = ctx.attrs.dep[AnonNonCbpInfo].artifact

    # Content-based script
    script = ctx.actions.declare_output("script.py", has_content_based_path = True)
    script = ctx.actions.write(
        script,
        [
            "import sys",
            "with open(sys.argv[1], 'w') as f:",
            "  f.write(sys.argv[2])",
        ],
    )

    # Content-based output
    out = ctx.actions.declare_output("out", has_content_based_path = True)
    args = cmd_args(["fbpython", script, out.as_output(), "hello world"])
    args.add(cmd_args(hidden = anon_artifact))

    ctx.actions.run(
        args,
        category = "test_run",
        expect_eligible_for_dedupe = True,
        prefer_remote = True,
    )

    return [DefaultInfo(default_output = out)]

run_with_anon_non_cbp_dep = rule(
    impl = _run_with_anon_non_cbp_dep_impl,
    attrs = {
        "dep": attrs.dep(),
    },
)

def _not_eligible_for_dedupe_impl(ctx) -> list[Provider]:
    script = ctx.actions.write(
        "script.py",
        [
            "import sys",
            "with open(sys.argv[1], 'w') as f:",
            "  f.write('hello')",
        ],
        has_content_based_path = False,
    )

    out = ctx.actions.declare_output("out", has_content_based_path = ctx.attrs.run_action_output_has_content_based_path)
    args = cmd_args(["fbpython", script, out.as_output()])

    ctx.actions.run(
        args,
        category = "test_run",
        expect_eligible_for_dedupe = ctx.attrs.expect_eligible_for_dedupe,
    )

    return [DefaultInfo(out)]

not_eligible_for_dedupe = rule(
    impl = _not_eligible_for_dedupe_impl,
    attrs = {
        "expect_eligible_for_dedupe": attrs.bool(default = False),
        "run_action_output_has_content_based_path": attrs.bool(default = True),
    },
)

def _failing_run_with_content_based_path_impl(ctx):
    script = ctx.actions.write(
        "script.py",
        [
            "import sys",
            "sys.exit(1)",
        ],
        has_content_based_path = True,
    )

    out = ctx.actions.declare_output("out", has_content_based_path = True)
    args = cmd_args(["fbpython", script, out.as_output()])

    ctx.actions.run(
        args,
        category = "test_run",
    )

    return [DefaultInfo(default_output = out), RunInfo(args = [out])]

failing_run_with_content_based_path = rule(
    impl = _failing_run_with_content_based_path_impl,
    attrs = {},
)

def _non_content_based_exec_dep_impl(ctx):
    script = ctx.actions.write(
        "script.py",
        [
            "import sys",
            "with open(sys.argv[1], 'w') as f:",
            "  f.write('hello world')",
        ],
        has_content_based_path = False,
    )
    out = ctx.actions.declare_output("out", has_content_based_path = True)
    ctx.actions.run(
        cmd_args(["fbpython", script, out.as_output()]),
        category = "test_exec_dep",
        # This is ignored for exec deps
        expect_eligible_for_dedupe = True,
    )
    return [DefaultInfo(default_output = out)]

non_content_based_exec_dep = rule(
    impl = _non_content_based_exec_dep_impl,
    attrs = {},
)

# Fixture for the symlink dedupe-eligibility blind spot (T276504188).
#
# The consuming `run` action depends on a symlink whose OWN path is
# content-based, but which points at a NON-content-based (configuration-based)
# artifact. A `run` genuinely materializes the symlink (and therefore its
# target), so the configuration-based target really is part of the action's
# resolved inputs. The dedupe-eligibility check only looks at each input
# artifact's own path and never follows the symlink to its target, so it
# wrongly reports the consuming action as eligible for dedupe even though its
# resolved input still embeds a configuration hash.
def _run_with_symlink_to_non_cbp_input_impl(ctx):
    # Non-content-based: its on-disk path embeds a configuration hash, so any
    # action that (transitively) depends on it is NOT eligible for dedupe.
    non_cbp = ctx.actions.write("non_cbp", "non_cbp", has_content_based_path = False)

    # Symlink whose own path is content-based, but which points at the
    # non-content-based artifact above.
    symlink = ctx.actions.symlink_file("symlink", non_cbp, has_content_based_path = True)

    # Content-based script that reads the symlink, forcing the symlink (and its
    # target) to actually be materialized as an input to the run action.
    script = ctx.actions.declare_output("script.py", has_content_based_path = True)
    script = ctx.actions.write(
        script,
        [
            "import sys",
            "with open(sys.argv[2], 'w') as out:",
            "  with open(sys.argv[1]) as inp:",
            "    out.write(inp.read())",
        ],
    )

    # Consuming run action. Its top-level inputs (script + symlink) are both
    # content-based, so the eligibility check passes them; the non-content-based
    # target is reachable only *through* the symlink, so the check never sees it.
    out = ctx.actions.declare_output("out", has_content_based_path = True)
    ctx.actions.run(
        cmd_args(["fbpython", script, symlink, out.as_output()]),
        category = "test_run",
    )

    return [DefaultInfo(default_output = out)]

run_with_symlink_to_non_cbp_input = rule(
    impl = _run_with_symlink_to_non_cbp_input_impl,
    attrs = {},
)

def _uses_exec_dep_impl(ctx):
    dep_out = ctx.attrs.exec_dep[DefaultInfo].default_outputs[0]
    script = ctx.actions.write(
        "script.py",
        [
            "import shutil",
            "import sys",
            "shutil.copyfile(sys.argv[1], sys.argv[2])",
        ],
        has_content_based_path = True,
    )
    out = ctx.actions.declare_output("out", has_content_based_path = True)
    ctx.actions.run(
        cmd_args(["fbpython", script, dep_out, out.as_output()]),
        category = "test_uses_exec_dep",
    )
    return [DefaultInfo(default_output = out)]

uses_exec_dep = rule(
    impl = _uses_exec_dep_impl,
    attrs = {
        "exec_dep": attrs.exec_dep(),
    },
)

def _ignore_artifacts_content_based_artifact_impl(ctx):
    stamp = ctx.actions.declare_output("stamp.txt", has_content_based_path = True)
    stamp = ctx.actions.write(stamp, ctx.attrs.data)

    script = ctx.actions.declare_output("script.py", has_content_based_path = True)
    script = ctx.actions.write(
        script,
        [
            "import os",
            "import sys",
            "assert not os.path.exists(sys.argv[2]), 'ignored input was materialized: ' + sys.argv[2]",
            "with open(sys.argv[1], 'w') as f:",
            "  f.write(sys.argv[2])",
        ],
    )

    out = ctx.actions.declare_output("out", has_content_based_path = True)
    args = cmd_args([
        "fbpython",
        script,
        out.as_output(),
        cmd_args(stamp, ignore_artifacts = True),
    ])
    ctx.actions.run(args, category = "test_run", prefer_remote = True)
    return [DefaultInfo(default_output = out)]

ignore_artifacts_content_based_artifact = rule(
    impl = _ignore_artifacts_content_based_artifact_impl,
    attrs = {
        "data": attrs.string(),
    },
)

def _ignore_artifacts_skips_failing_static_path_dep_impl(ctx):
    broken = ctx.actions.declare_output("broken.txt", has_content_based_path = False)
    ctx.actions.run(
        cmd_args(["fbpython", "-c", "import sys; sys.exit(1)", broken.as_output()]),
        category = "broken",
    )

    out = ctx.actions.declare_output("out")
    ctx.actions.run(
        cmd_args([
            "fbpython",
            "-c",
            "import sys; open(sys.argv[1], 'w').write(sys.argv[2])",
            out.as_output(),
            cmd_args(broken, ignore_artifacts = True),
        ]),
        category = "test_run",
    )
    return [DefaultInfo(default_output = out)]

ignore_artifacts_skips_failing_static_path_dep = rule(
    impl = _ignore_artifacts_skips_failing_static_path_dep_impl,
    attrs = {},
)

def _ignore_artifacts_rejects_hidden_content_based_artifact_impl(ctx):
    stamp = ctx.actions.declare_output("stamp.txt", has_content_based_path = True)
    stamp = ctx.actions.write(stamp, "stamp")

    out = ctx.actions.declare_output("out")
    ctx.actions.run(
        cmd_args([
            "fbpython",
            "-c",
            "import sys; open(sys.argv[1], 'w').write('out')",
            out.as_output(),
            cmd_args(hidden = stamp, ignore_artifacts = True),
        ]),
        category = "test_run",
    )
    return [DefaultInfo(default_output = out)]

ignore_artifacts_rejects_hidden_content_based_artifact = rule(
    impl = _ignore_artifacts_rejects_hidden_content_based_artifact_impl,
    attrs = {},
)

def _ignore_artifacts_nested_under_hidden_impl(ctx):
    broken = ctx.actions.declare_output("broken.txt", has_content_based_path = True)
    ctx.actions.run(
        cmd_args(["fbpython", "-c", "import sys; sys.exit(1)", broken.as_output()]),
        category = "broken",
    )

    out = ctx.actions.declare_output("out")
    args = cmd_args(
        [
            "fbpython",
            "-c",
            "import sys; open(sys.argv[1], 'w').write('out')",
            out.as_output(),
        ],
        # Someone else's hidden list around an ignore_artifacts scope: nothing
        # renders the artifact, so it is not a dependency and is never built.
        hidden = cmd_args(broken, ignore_artifacts = True),
    )
    ctx.actions.run(args, category = "test_run")
    return [DefaultInfo(default_output = out)]

ignore_artifacts_nested_under_hidden = rule(
    impl = _ignore_artifacts_nested_under_hidden_impl,
    attrs = {},
)

def _salt_hidden_dependency_impl(ctx):
    stamp = ctx.actions.write("stamp.txt", ctx.attrs.data, has_content_based_path = False)

    script = ctx.actions.write(
        "script.py",
        [
            "import os",
            "import sys",
            "import uuid",
            "assert not os.path.exists(sys.argv[2]), 'ignored input was materialized: ' + sys.argv[2]",
            "with open(sys.argv[1], 'w') as f:",
            "  f.write(uuid.uuid4().hex)",
        ],
    )

    out = ctx.actions.declare_output("out")
    args = cmd_args([
        "fbpython",
        script,
        out.as_output(),
        # The path is passed only so the script can assert it was not materialized.
        cmd_args(stamp, ignore_artifacts = True),
        # The content dependency: nothing rendered, built but not materialized.
        cmd_args(hidden = stamp, experimental_salt_hidden = True),
    ])
    ctx.actions.run(args, category = "test_run", prefer_remote = True)
    return [DefaultInfo(default_output = out)]

salt_hidden_dependency = rule(
    impl = _salt_hidden_dependency_impl,
    attrs = {
        "data": attrs.string(),
    },
)
