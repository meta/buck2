# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

load("@prelude//utils:utils.bzl", "value_or")
load(":exec_deps.bzl", "HttpArchiveExecDeps")
load(":unarchive_legacy.bzl", legacy_extract_archive = "extract_archive")

_ARCHIVE_EXTS = [
    "tar",
    "tar.bz2",
    "tar.gz",
    "tar.xz",
    "tar.zst",
    "zip",
]

def _url_path(url: str) -> str:
    if "?" in url:
        return url.split("?")[0]
    else:
        return url

def _type_from_url(url: str) -> [str, None]:
    url_path = _url_path(url)
    for filename_ext in _ARCHIVE_EXTS:
        if url_path.endswith("." + filename_ext):
            return filename_ext
    return None

def archive_type(url_or_path: str, typ: str | None) -> str:
    if typ == None:
        typ = value_or(_type_from_url(url_or_path), "tar.gz")
    if typ not in _ARCHIVE_EXTS:
        fail("unsupported archive type: {}".format(typ))
    return typ

def unarchive(
    ctx: AnalysisContext,
    archive: Artifact,
    output_name: str,
    ext_type: str,
    excludes: list[str],
    strip_prefix: str | None,
    exec_deps: HttpArchiveExecDeps,
    prefer_local: bool,
    sub_targets: list[str] | dict[str, list[str]],
    has_content_based_path: bool = False,
):
    if read_root_config("buck2", "extract_archives_in_daemon", "false") == "true":
        output = ctx.actions.extract_archive(
            output_name,
            archive,
            type = ext_type,
            strip_prefix = strip_prefix,
            excludes = excludes,
            has_content_based_path = has_content_based_path,
        )
    else:
        output = legacy_extract_archive(
            ctx,
            archive = archive,
            output_name = output_name,
            ext_type = ext_type,
            excludes = excludes,
            strip_prefix = strip_prefix,
            exec_deps = exec_deps,
            prefer_local = prefer_local,
            has_content_based_path = has_content_based_path,
        )

    if type(sub_targets) == type([]):
        sub_targets = {path: [DefaultInfo(default_output = output.project(path))] for path in sub_targets}
    elif type(sub_targets) == type({}):
        sub_targets = {name: [DefaultInfo(default_outputs = [output.project(path) for path in paths])] for name, paths in sub_targets.items()}
    else:
        fail("sub_targets must be a list or dict")

    return output, sub_targets
