# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

def _write_impl(ctx):
    # NOTE: This uses a run action so that we can exercise local uploads.
    tmp = ctx.actions.write("tmp", ctx.attrs.text, has_content_based_path = False)

    out = ctx.actions.declare_output("out", dir = ctx.attrs.dir, has_content_based_path = False)
    ctx.actions.run(
        [
            "fbpython",
            "-c",
            "import sys, os, shutil; dirname= os.path.dirname(sys.argv[2]);\nif not os.path.exists(dirname): os.makedirs(dirname);\nshutil.copyfile(sys.argv[1],sys.argv[2]);",
            tmp,
            cmd_args(out.as_output(), format = ctx.attrs.format),
        ],
        category = "cp",
        local_only = ctx.attrs.local_only,
        allow_cache_upload = ctx.attrs.allow_cache_upload,
    )

    return [DefaultInfo(default_output = out)]

write = rule(
    impl = _write_impl,
    attrs = {
        "allow_cache_upload": attrs.option(attrs.bool(), default = None),
        "dir": attrs.bool(default = False),
        "format": attrs.string(default = "{}"),
        "local_only": attrs.bool(default = False),
        "text": attrs.string(),
    },
)
