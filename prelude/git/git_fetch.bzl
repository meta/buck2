# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

load("@prelude//:prelude_rule.bzl", "prelude_rule")
load("@prelude//decls:common.bzl", "buck")
_HEX_DIGITS = ["0", "1", "2", "3", "4", "5", "6", "7", "8", "9", "a", "b", "c", "d", "e", "f"]

def _is_hex(rev: str, *, length: int) -> bool:
    if len(rev) != length:
        return False
    for digit in rev.elems():
        if digit not in _HEX_DIGITS:
            return False
    return True

def git_fetch_impl(ctx: AnalysisContext) -> list[Provider]:
    object_format = ctx.attrs.object_format
    rev = ctx.attrs.rev
    if object_format == None:
        if not _is_hex(rev, length = 40):
            fail("git_fetch's `rev` must be a 40-hex-digit commit hash: {}".format(rev))
    elif object_format == "sha1":
        if not _is_hex(rev, length = 40):
            fail("git_fetch's `rev` must be a 40-hex-digit commit hash when the chosen object format is sha1: {}".format(rev))
    elif object_format == "sha256":
        if not _is_hex(rev, length = 64):
            fail("git_fetch's `rev` must be a 64-hex-digit commit hash when the chosen object format is sha256: {}".format(rev))
    else:
        fail("Invalid git_fetch `object_format`: Must be one of sha1 or sha256: {}".format(object_format))

    git_dir = ctx.actions.declare_output(".git", dir = True, has_content_based_path = False)

    short_path = ctx.attrs.name.removesuffix(".git")
    if not short_path:
        short_path = "work-tree"
    work_tree = ctx.actions.declare_output(short_path, dir = True, has_content_based_path = False)

    if ctx.attrs.update_submodules:
        update_submodules = cmd_args("--update-submodules")
    else:
        update_submodules = cmd_args()

    cmd = [
        ctx.attrs._git_fetch_tool[RunInfo],
        cmd_args("--git-dir=", git_dir.as_output(), delimiter = ""),
        cmd_args("--work-tree=", work_tree.as_output(), delimiter = ""),
        cmd_args("--repo=", ctx.attrs.repo, delimiter = ""),
        cmd_args("--rev=", rev, delimiter = ""),
        update_submodules,
    ]
    if ctx.attrs.git != None:
        cmd.append(cmd_args("--git=", ctx.attrs.git, delimiter = ""))
    if object_format != None:
        cmd.append(cmd_args("--object-format=", object_format, delimiter = ""))

    ctx.actions.run(
        cmd,
        category = "git_fetch",
        local_only = True,
        allow_cache_upload = ctx.attrs.allow_cache_upload,
    )

    sub_targets = {path: [DefaultInfo(default_output = work_tree.project(path))] for path in ctx.attrs.sub_targets}

    # The repository itself, for consumers that need to serve the fetch as a local git remote
    # rather than read the checked-out files. git refuses to track a path named ".git", so no
    # requested work-tree projection can ever claim this key.
    sub_targets[".git"] = [DefaultInfo(default_output = git_dir)]

    return [
        DefaultInfo(
            default_output = work_tree,
            sub_targets = sub_targets,
        )
    ]

git_fetch = prelude_rule(
    name = "git_fetch",
    impl = git_fetch_impl,
    docs = """
        Checkout a commit from a git repository.
    """,
    examples = """
        ```
        git_fetch(
            name = "serde.git",
            repo = "https://github.com/serde-rs/serde",
            rev = "fccb9499bccbaca0b7eef91a3a82dfcb31e0b149",
        )
        ```
    """,
    further = None,
    attrs = (
        # @unsorted-dict-items
        {
            "allow_cache_upload": attrs.bool(
                doc = """
                Whether the results of the fetch can be written to the action cache and CAS.
            """,
                default = True,
            ),
            "git": attrs.option(
                attrs.string(),
                default = None,
                doc = """
                Path to the git binary to use. If not specified, the system
                git from PATH is used, with fallback to common system
                locations (/usr/bin/git, /usr/local/bin/git).
            """,
            ),
            "object_format": attrs.option(
                attrs.enum(["sha1", "sha256"]),
                default = None,
                doc = """
                The object format to use for the underlying Git repository.
                Must be one of `sha1` or `sha256`.
                """,
            ),
            "repo": attrs.string(
                doc = """
                Url suitable as a git remote.
            """
            ),
            "rev": attrs.string(
                doc = """
                Commit hash. 40 hex digits for sha1, 64 hex digits for sha256.
            """
            ),
            "update_submodules": attrs.bool(
                default = False,
                doc = """
                Whether to initialize and update Git submodules after the worktree checkout.
                If true, submodules are fetched recursively with depth 1.
            """,
            ),
            "sub_targets": attrs.list(
                attrs.string(),
                default = [],
                doc = """
                A list of paths within the remote repo to be made accessible as sub-targets.
                For example if we have a git_fetch with `name = "serde.git"` and
                `sub_targets = ["serde_derive"]`, then other targets would be able to refer
                to the serde_derive subdirectory of the repo as `":serde.git[serde_derive]"`.
            """,
            ),
            "_git_fetch_tool": attrs.default_only(attrs.exec_dep(providers = [RunInfo], default = "prelude//git/tools:git_fetch")),
        }
        | buck.licenses_arg()
        | buck.labels_arg()
        | buck.contacts_arg()
    ),
)
