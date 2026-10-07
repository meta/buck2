# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

# The script lists every file below `sys.argv[1]`. A local action reads the source tree on disk.
# The action writes its own output below `buck-out`, so the script skips `buck-out`.
_LIST_FILES = """
import os
import sys

root = sys.argv[1]
paths = []
for dirpath, dirnames, filenames in os.walk(root):
    if os.path.samefile(dirpath, root) and "buck-out" in dirnames:
        dirnames.remove("buck-out")
    for name in filenames:
        paths.append(os.path.relpath(os.path.join(dirpath, name), root))
with open(sys.argv[2], "w") as f:
    f.write("".join(path + "\\n" for path in sorted(paths)))
"""

def _list_files_impl(ctx: AnalysisContext):
    src = ctx.attrs.src
    if ctx.attrs.copy:
        # `copy_dir` creates the copy from the recorded digest, so the listing shows the entries
        # of the digest.
        src = ctx.actions.copy_dir("copied", src, has_content_based_path = False)
    out = ctx.actions.declare_output("out", has_content_based_path = False)
    ctx.actions.run(
        cmd_args(
            "fbpython",
            "-c",
            _LIST_FILES,
            src,
            out.as_output(),
            hidden = ctx.attrs.extra_srcs,
        ),
        category = "list_files",
        local_only = True,
    )
    return [DefaultInfo(default_output = out)]

list_files = rule(
    impl = _list_files_impl,
    attrs = {
        "copy": attrs.bool(default = False),
        "extra_srcs": attrs.list(attrs.source(), default = []),
        "src": attrs.source(allow_directory = True),
    },
)

def _write_path_impl(ctx: AnalysisContext):
    out = ctx.actions.declare_output("out", has_content_based_path = False)
    ctx.actions.run(
        cmd_args(
            "fbpython",
            "-c",
            "import sys; open(sys.argv[2], 'w').write(sys.argv[1])",
            ctx.attrs.src,
            out.as_output(),
        ),
        category = "write_path",
        local_only = True,
    )
    return [DefaultInfo(default_output = out)]

write_path = rule(
    impl = _write_path_impl,
    attrs = {
        "src": attrs.source(allow_directory = True),
    },
)
