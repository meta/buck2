# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

load("@prelude//:prelude_rule.bzl", "prelude_rule")
load("@prelude//decls:common.bzl", "buck")
load("@prelude//utils:expect.bzl", "expect")

WorkerToolInfo = provider(
    fields = {
        "command": provider_field(typing.Any, default = None),  # cmd_args
    },
)

def worker_tool_impl(ctx: AnalysisContext) -> list[Provider]:
    """
     worker_tool() rule implementation

    Args:
        ctx: rule analysis context
    Returns:
        list of created providers (DefaultInfo with an empty output and TemplatePlaceholderInfo with $(worker) macro key)
    """

    executable = ctx.attrs.exe
    worker_tool_run_info = executable[RunInfo]
    expect(worker_tool_run_info != None, "Worker tool executable must have a RunInfo!")

    worker_tool_runner = ctx.attrs._worker_tool_runner[RunInfo]
    worker_tool_cmd = [worker_tool_runner]
    worker_tool_cmd.append("--worker-tool")
    worker_tool_cmd.append(worker_tool_run_info)

    worker_args = ctx.attrs.args
    if worker_args:
        worker_args_file, _ = ctx.actions.write(
            "worker_tool_args",
            worker_args,
            allow_args = True,
            has_content_based_path = False,
        )

        worker_tool_cmd.append("--worker-args-file")
        worker_tool_cmd.append(worker_args_file)

    worker_env = ctx.attrs.env
    if worker_env:
        env_args = []
        for key, value in worker_env.items():
            env_args.append(key)
            env_args.append(value)

        env_args_file, _ = ctx.actions.write(
            "worker_tool_envs",
            env_args,
            allow_args = True,
            has_content_based_path = False,
        )

        worker_tool_cmd.append("--worker-env-file")
        worker_tool_cmd.append(env_args_file)

    worker_tool_cmd = cmd_args(worker_tool_cmd)
    return [
        DefaultInfo(),
        RunInfo(
            args = worker_tool_cmd,
        ),
        WorkerToolInfo(
            command = worker_tool_cmd,
        ),
    ]

worker_tool = prelude_rule(
    name = "worker_tool",
    impl = worker_tool_impl,
    docs = """
        Some external tools have high startup costs. To amortize those costs over the whole build
         rather than paying them for each rule invocation, use the `worker_tool()` rule
         in conjunction with `genrule()`.
         Buck then starts the external tool once and reuses it by communicating with it
         over `stdin` and `stdout` using a simple JSON protocol.

         A `worker_tool` rule can be referenced in the `cmd` parameter of
         a `genrule` by using the macro:

        ```
        $(exe //path/to:target)
        ```
    """,
    examples = """
        Consider the following `build rules`:

        ```
        #
        # Buck
        #
        worker_tool(
          name = 'ExternalToolWorker',
          exe = ':ExternalTool',
          args = '--arg1 --arg2'
        )

        sh_binary(
          name = 'ExternalTool',
          main = 'external_tool.sh',
        )

        genrule(
          name = 'TransformA',
          out = 'OutputA.txt',
          cmd = '$(exe :ExternalToolWorker) argA',
        )

        genrule(
          name = 'TransformB',
          out = 'OutputB.txt',
          cmd = '$(exe :ExternalToolWorker) argB',
        )

        genrule(
          name = 'TransformC',
          out = 'OutputC.txt',
          cmd = '$(exe :ExternalToolWorker) argC',
        )
        ```

         When doing a `buck build` on all three of the above `genrules`, Buck
         first creates the worker process by invoking:

        ```
        ./external_tool.sh --arg1 --arg2
        ```

         Buck then communicates with this process using JSON over `stdin`,
         starting with a handshake:

        ```
        [
          {
            "id": 0,
            "type": "handshake",
            "protocol_version": "0",
            "capabilities": []
          }
        ```

         Buck then waits for the tool to reply on `stdout`:

        ```
        [
          {
            "id": 0,
            "type": "handshake",
            "protocol_version": "0",
            "capabilities": []
          }
        ```

         Then, when building the first `genrule`, Buck writes to `stdin`:

        ```
          ,{
            "id": 1,
            "type": "command",
            "args_path": "/tmp/1.args",
            "stdout_path": "/tmp/1.out",
            "stderr_path": "/tmp/1.err"
          }
        ```

         The file `/tmp/1.args` contains `argA`. The tool should
         perform the necessary work for this job and then write the job's output to the files
         supplied by Buck—in this case, `/tmp/1.out` and `/tmp/1.err`.
         Once the job is done, the tool should reply to Buck on `stdout` with:

        ```
          ,{
            "id": 1,
            "type": "result",
            "exit_code": 0
          }
        ```

         Once Buck hears back from the first genrule's job, it submits the second genrule's job in the
         same fashion and awaits the response. When the build is all finished,
         Buck closes the JSON by writing to `stdin`:

        ```
        ]
        ```

         which signals the tool that it should exit after replying on `stdout` with:

        ```
        ]
        ```

         In this example, Buck is guaranteed to invoke

        ```
        ./external_tool.sh --arg1 --arg2
        ```

         only once during the build. The three jobs corresponding to the three genrules are submitted
         synchronously to the single worker process.

         Note that the `id` values in the messages are not necessarily increasing or sequential,
         but they do have to match between the request message and the response message of a given job as
         well as in the initial handshake.

         If the tool receives a message type it cannot interpret it should answer with:

        ```
        {
          "id": &ltn>,
          "type": "error",
          "exit_code": 1
        }
        ```

         If the tool receives a message type it can interpret, but the other attributes of the
         message are in an inconsistent state, it should answer with:

        ```
        {
          "id": &ltn>,
          "type": "error",
          "exit_code": 2
        }
        ```
    """,
    further = None,
    attrs = (
        # @unsorted-dict-items
        {
            "exe": attrs.option(
                attrs.dep(),
                default = None,
                doc = """
                A `build target` for a rule that outputs
                 an executable, such as an `sh_binary()`.
                 Buck runs this executable only once per build.
            """,
            ),
            "args": attrs.one_of(
                attrs.arg(),
                attrs.list(attrs.arg()),
                default = [],
                doc = """
                A string of args that is passed to the executable represented by `exe` on
                 initial startup.
            """,
            ),
            "max_workers": attrs.option(
                attrs.int(),
                default = None,
                doc = """
                The maximum number of workers of this type that Buck starts. Use `-1` to allow
                 the creation of as many workers as necessary.
            """,
            ),
            "max_workers_per_thread_percent": attrs.option(
                attrs.int(),
                default = None,
                doc = """
                The maximum ratio of workers of this type that Buck starts per
                 thread, specified as a positive integer percentage (1-100). Must be
                 greater than or equal to `1` and less than or equal to `100`.
                 Only one of `max_workers` and `max_workers_per_thread_percent` may be specified.
            """,
            ),
            "env": attrs.dict(
                key = attrs.string(),
                value = attrs.arg(),
                sorted = False,
                default = {},
                doc = """
                A map of environment variables that is passed to the executable represented
                 by `exe` on initial startup.
            """,
            ),
            "persistent": attrs.option(
                attrs.bool(),
                default = None,
                doc = """
                If set to true, Buck does not restart the tool unless the tool itself changes. This means the
                 tool persists across multiple Buck commands without being shut down and may see the same
                 rule being built more than once. Be careful not to use this setting with tools that don't expect
                 to process the same input—with different contents—twice!
            """,
            ),
            # FIXME: prelude// should be standalone (not refer to fbsource//)
            "_worker_tool_runner": attrs.default_only(attrs.dep(default = "prelude//js/worker_runner:worker_tool_runner")),
        }
        | buck.licenses_arg()
        | buck.labels_arg()
        | buck.contacts_arg()
    ),
)
