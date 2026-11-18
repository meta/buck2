# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

_ALLOW_DEP_FILE_CACHE_UPLOAD = read_config("test", "allow_dep_file_cache_upload") in ["true", "True"]
_ALLOW_CACHE_UPLOAD = read_config("test", "allow_cache_upload") in ["true", "True"]

def _tag_files(tag, files):
    return [tag.tag_artifacts(f) for f in files]

def _get_tagged_artifacts(
    ctx, dep_file: Artifact, used_files: list[Artifact], unused_files: list[Artifact]
) -> (ArtifactTag, list[typing.Any], list[typing.Any], typing.Any):
    tag = ctx.actions.artifact_tag()
    tagged_used_files = _tag_files(tag, used_files)
    tagged_unused_files = _tag_files(tag, unused_files)
    tagged_dep_file = tag.tag_artifacts(dep_file.as_output())
    return (tag, tagged_used_files, tagged_unused_files, tagged_dep_file)

def _with_two_dep_files_impl(ctx):
    allow_dep_file_cache_upload = _ALLOW_DEP_FILE_CACHE_UPLOAD
    allow_cache_upload = _ALLOW_CACHE_UPLOAD

    out = ctx.actions.declare_output(ctx.attrs.out_name, has_content_based_path = True)

    (dep_file_name0, used_files0, unused_files0) = ctx.attrs.dep_file_contents[0]
    dep_file0 = ctx.actions.declare_output(dep_file_name0, has_content_based_path = True)
    (tag0, tagged_used_files0, tagged_unused_files0, tagged_dep_file0) = _get_tagged_artifacts(ctx, dep_file0, used_files0, unused_files0)

    (dep_file_name1, used_files1, unused_files1) = ctx.attrs.dep_file_contents[1]
    dep_file1 = ctx.actions.declare_output(dep_file_name1, has_content_based_path = True)
    (tag1, tagged_used_files1, tagged_unused_files1, tagged_dep_file1) = _get_tagged_artifacts(ctx, dep_file1, used_files1, unused_files1)

    cmd = [
        "fbpython",
        ctx.attrs.create_dep_file,
        "--out",
        out.as_output(),
        "--dep-file0",
        tagged_dep_file0,
        "--used-files0",
        tagged_used_files0,
        "--dep-file1",
        tagged_dep_file1,
        "--used-files1",
        tagged_used_files1,
    ]
    if ctx.attrs.fail:
        cmd = cmd + ["--fail"]

    cmd = cmd_args(
        cmd,
        # Add them to the command so they are tracked as inputs but don't do anything with them.
        hidden = tagged_unused_files0 + tagged_unused_files1 + ctx.attrs.untagged_files,
    )

    ctx.actions.run(
        cmd,
        category = "create_dep_file",
        dep_files = {"dep_file_tag0": tag0, "dep_file_tag1": tag1},
        allow_cache_upload = allow_cache_upload,
        allow_dep_file_cache_upload = allow_dep_file_cache_upload,
        env = {"cache_buster": ctx.attrs.cache_buster},
    )

    return [
        DefaultInfo(
            default_output = out,
            sub_targets = {dep_file_name0: [DefaultInfo(default_output = dep_file0)], dep_file_name1: [DefaultInfo(default_output = dep_file1)]},
        ),
    ]

with_two_dep_files = rule(
    attrs = {
        "cache_buster": attrs.string(default = read_config("test", "cache_buster", "")),
        "create_dep_file": attrs.source(),
        "dep_file_contents": attrs.list(
            attrs.tuple(
                attrs.string(),  # dep file name
                attrs.list(attrs.source()),  # tagged files to show up in the dep file
                attrs.list(attrs.source()),  # tagged files to not show up in the dep file
            ),
        ),
        "fail": attrs.bool(default = False),
        "out_name": attrs.string(),
        "untagged_files": attrs.list(attrs.source()),
    },
    impl = _with_two_dep_files_impl,
)

def _write_impl(ctx):
    out = ctx.actions.write(ctx.attrs.out, ctx.attrs.content, has_content_based_path = True)
    return [
        DefaultInfo(default_output = out),
    ]

write = rule(
    attrs = {
        "content": attrs.string(),
        "out": attrs.string(),
    },
    impl = _write_impl,
)
