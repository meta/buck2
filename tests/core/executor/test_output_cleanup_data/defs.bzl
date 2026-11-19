# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

def _action_impl(ctx):
    isdir = ctx.attrs.isdir
    out = ctx.actions.declare_output("out", dir = isdir, has_content_based_path = False)

    ctx.actions.run(
        [
            "fbpython",
            "-c",
            "\n".join([
                "import os, sys",
                "path, isdir, seed, mode = sys.argv[1:]",
                "isdir = int(isdir)",
                "permtarget = path",
                "if isdir:",
                "    permtarget = path",
                "    os.makedirs(path, exist_ok=True)",
                "    path = path + os.path.sep + 'out.txt'",
                "with open(path, 'w') as f:",
                "    f.write(seed)",
                "if mode != 'None':",
                "    os.chmod(permtarget, int(mode))",
            ]),
            out.as_output(),
            str(int(ctx.attrs.isdir)),
            ctx.attrs.seed,
            str(ctx.attrs.mode),
        ],
        local_only = ctx.attrs.local_only,
        category = "write",
    )

    return [DefaultInfo(default_output = out)]

action = rule(
    attrs = {
        "isdir": attrs.bool(default = False),
        "local_only": attrs.bool(),
        "mode": attrs.option(attrs.int(), default = None),
        "seed": attrs.string(),
    },
    impl = _action_impl,
)

def _symlinked_dir_impl(ctx):
    f = ctx.actions.write("dst/f", "file", has_content_based_path = False)
    out = ctx.actions.symlinked_dir("out", {ctx.attrs.seed: f}, has_content_based_path = False)
    return [DefaultInfo(default_output = out)]

symlinked_dir = rule(
    attrs = {
        "seed": attrs.string(),
    },
    impl = _symlinked_dir_impl,
)

def _write_impl(ctx):
    out = ctx.actions.write("out", ctx.attrs.seed, has_content_based_path = False)
    return [DefaultInfo(default_output = out)]

write = rule(
    attrs = {
        "seed": attrs.string(),
    },
    impl = _write_impl,
)

def _copy_impl(ctx):
    f = ctx.actions.write("dst/f", ctx.attrs.seed, has_content_based_path = False)
    out = ctx.actions.copy_file("out", f, has_content_based_path = False)
    return [DefaultInfo(default_output = out)]

copy = rule(
    attrs = {
        "seed": attrs.string(),
    },
    impl = _copy_impl,
)

def declare_targets():
    target = read_config("test", "main")

    if target == "local_action-a":
        action(name = "main", local_only = True, seed = "local-action-a")
    elif target == "local_action-b":
        action(name = "main", local_only = True, seed = "local-action-b")
    elif target == "remote_action-a":
        action(name = "main", local_only = False, seed = "remote-action-a")
    elif target == "remote_action-b":
        action(name = "main", local_only = False, seed = "remote-action-b")
    elif target == "symlinked_dir-a":
        symlinked_dir(name = "main", seed = "symlinked_dir-a")
    elif target == "symlinked_dir-b":
        symlinked_dir(name = "main", seed = "symlinked_dir-b")
    elif target == "write-a":
        write(name = "main", seed = "write-a")
    elif target == "write-b":
        write(name = "main", seed = "write-b")
    elif target == "copy-a":
        copy(name = "main", seed = "copy-a")
    elif target == "copy-b":
        copy(name = "main", seed = "copy-b")
    elif target == "local_readonly_file-a":
        action(name = "main", mode = 0o400, local_only = True, seed = target)
    elif target == "local_readonly_file-b":
        action(name = "main", mode = 0o400, local_only = True, seed = target)
    elif target == "local_readonly_dir-a":
        action(name = "main", isdir = True, mode = 0o500, local_only = True, seed = target)
    elif target == "local_readonly_dir-b":
        action(name = "main", isdir = True, mode = 0o500, local_only = True, seed = target)
    elif target == "local_nonexec_dir-a":
        action(name = "main", isdir = True, mode = 0o000, local_only = True, seed = target)
    elif target == "local_nonexec_dir-b":
        action(name = "main", isdir = True, mode = 0o000, local_only = True, seed = target)
    elif target == "remote_readonly_file-a":
        action(name = "main", mode = 0o400, local_only = False, seed = target)
    elif target == "remote_readonly_file-b":
        action(name = "main", mode = 0o400, local_only = False, seed = target)
    elif target == "remote_readonly_dir-a":
        action(name = "main", isdir = True, mode = 0o500, local_only = False, seed = target)
    elif target == "remote_readonly_dir-b":
        action(name = "main", isdir = True, mode = 0o500, local_only = False, seed = target)
    elif target == "remote_nonexec_dir-a":
        action(name = "main", isdir = True, mode = 0o000, local_only = False, seed = target)
    elif target == "remote_nonexec_dir-b":
        action(name = "main", isdir = True, mode = 0o000, local_only = False, seed = target)
    else:
        fail("Invalid target: `{}`".format(target))
