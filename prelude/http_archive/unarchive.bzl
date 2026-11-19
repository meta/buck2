# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

load("@prelude//os_lookup:defs.bzl", "Os", "OsLookup")
load("@prelude//utils:expect.bzl", "expect")
load("@prelude//utils:utils.bzl", "value_or")
load(":exec_deps.bzl", "HttpArchiveExecDeps")

# Flags to apply to decompress the various types of archives.
_TAR_FLAGS = {
    "tar": [],
    "tar.bz2": ["-j"],
    "tar.gz": ["-z"],
    "tar.xz": ["-J"],
    "tar.zst": ["--use-compress-program=unzstd"],
}

_ARCHIVE_EXTS = _TAR_FLAGS.keys() + [
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

# Returns a two-element tuple:
#
# 1. The cmd_args with the unarchive command
# 2. A bool indicating whether the prefix still needs to be stripped (in cases where the tool used to uncompress does not support this feature).
def _unarchive_cmd(ext_type: str, archive: Artifact, strip_prefix: [str, None]) -> (cmd_args, bool):
    if ext_type in _TAR_FLAGS:
        return cmd_args(
            "tar",
            _TAR_FLAGS[ext_type],
            "-x",
            "-f",
            archive,
            _tar_strip_prefix_flags(strip_prefix),
        ), False
    elif ext_type == "zip":
        # gnutar does not intrinsically support zip
        return cmd_args("unzip", archive), bool(strip_prefix)
    else:
        fail()

def _tar_strip_prefix_flags(strip_prefix: [str, None]) -> list[str]:
    if strip_prefix:
        # count nonempty path components in the prefix
        count = len(filter(lambda c: c != "", strip_prefix.split("/")))
        return ["--strip-components=" + str(count), strip_prefix]
    return []

def _powershell_quote(arg) -> cmd_args:
    # A single quote in a PowerShell single-quoted string is written twice.
    return cmd_args(arg, replace_regex = ("'", "''"), format = "'{}'")

# buck-out on Windows on eden is a symlink. bsdtar which ships with Windows will
# not extract files when the cwd contains a path segment that is a symlink (as
# a side effect of or excessively cautious against wild writes to the system).
# Powershell lets us 'dereference' that symlink to get the physical path to the
# output folder we tell tar to unpack into while preserving security, instead of
# passing -P.
def _windows_unpack_ps1(out: OutputArtifact, archive: Artifact, ext_type: str, strip_prefix: [str, None], exclude_flags: list) -> list:
    tar = '"$env:SystemRoot\\System32\\tar.exe"'
    quoted_archive = _powershell_quote(archive)
    strip = [_powershell_quote(arg) for arg in _tar_strip_prefix_flags(strip_prefix)]
    quoted_exclude_flags = [_powershell_quote(arg) for arg in exclude_flags]

    lines = [
        "$ErrorActionPreference = 'Stop'",
        cmd_args(_powershell_quote(out), format = "$out = {}"),
        "New-Item -ItemType Directory -Force -Path $out | Out-Null",
        "$link = (Get-Item -LiteralPath 'buck-out').Target",
        "if ($link) { $real = Join-Path @($link)[0] $out.Substring('buck-out'.Length).TrimStart('\\', '/') } else { $real = $out }",
    ]

    if ext_type == "tar.zst":
        # bsdtar cannot invoke zstd itself, and a PowerShell native pipe would
        # corrupt the binary stream, so decompress to a scratch file first.
        return lines + [
            "$scratch = $env:BUCK_SCRATCH_PATH",
            "if (-not $scratch) { $scratch = [System.IO.Path]::GetTempPath() }",
            "$tmp = Join-Path $scratch 'http_archive_unpack.tar'",
            cmd_args("zstd", "-d", "-f", quoted_archive, "-o", '"$tmp"', delimiter = " "),
            cmd_args("&", tar, "-x", "-C", '"$real"', "-f", '"$tmp"', strip, quoted_exclude_flags, delimiter = " "),
            "Remove-Item -LiteralPath $tmp -Force",
        ]
    elif ext_type == "zip":
        return lines + [
            cmd_args("&", tar, "-x", "-C", '"$real"', "-f", quoted_archive, strip, delimiter = " "),
        ]
    elif ext_type in _TAR_FLAGS:
        return lines + [
            cmd_args("&", tar, _TAR_FLAGS[ext_type], "-x", "-C", '"$real"', "-f", quoted_archive, strip, quoted_exclude_flags, delimiter = " "),
        ]
    else:
        fail("unsupported archive type on Windows: {}".format(ext_type))

def unarchive(
    ctx: AnalysisContext,
    archive: Artifact,
    output_name: str,
    ext_type,
    excludes,
    strip_prefix,
    exec_deps: HttpArchiveExecDeps,
    prefer_local: bool,
    sub_targets: list[str] | dict[str, list[str]],
    has_content_based_path: bool = False,
):
    exec_is_windows = exec_deps.exec_os_type[OsLookup].os == Os("windows")

    # The excludes listing runs `tar --list` and redirects it to a file; keep it
    # in the shell whose redirect writes raw bytes (cmd on Windows, sh else).
    if exec_is_windows:
        listing_ext = "bat"
        listing_interpreter = []
        first_param = "%1"
    else:
        listing_ext = "sh"
        listing_interpreter = ["/bin/sh"]
        first_param = '"$1"'

    # Unpack archive to output directory.
    exclude_flags = []
    exclude_hidden = []
    if excludes:
        tar_flags = _TAR_FLAGS.get(ext_type)
        expect(tar_flags != None, "excludes not supported for non-tar archives")

        # Tar excludes files using globs, but we take regexes, so we need to
        # apply our regexes onto the file listing and produce an exclusion list
        # that just has strings.
        exclusions = ctx.actions.declare_output(output_name + "_exclusions", has_content_based_path = False)
        contents = ctx.actions.declare_output(output_name + "_contents", has_content_based_path = False)
        tar_script, _ = ctx.actions.write(
            "{}_listing.{}".format(output_name, listing_ext),
            [
                cmd_args(
                    archive,
                    format = "tar --list " + " ".join(tar_flags) + " -f {} > " + first_param,
                )
            ],
            is_executable = True,
            allow_args = True,
            has_content_based_path = False,
        )
        ctx.actions.run(
            cmd_args(listing_interpreter + [tar_script, contents.as_output()], hidden = [archive]),
            category = "process_exclusions",
        )

        def create_exclusion_list(ctx: AnalysisContext, artifacts, outputs):
            files = artifacts[contents].read_string().splitlines()
            exclusion_list = []
            exclude_regexen = [regex(e) for e in excludes]
            for f in files:
                for exclusion in exclude_regexen:
                    if exclusion.match(f):
                        exclusion_list.append(f)
                        break
            ctx.actions.write(outputs[exclusions], "\n".join(exclusion_list))

        ctx.actions.dynamic_output(
            dynamic = [contents],
            inputs = [],
            outputs = [exclusions.as_output()],
            f = create_exclusion_list,
        )

        exclude_flags.append(cmd_args(exclusions, format = "--exclude-from={}"))
        exclude_hidden.append(exclusions)

    unarchive_cmd = None  # unused on Windows (see _windows_unpack_ps1)
    if exec_is_windows:
        needs_strip_prefix = False
        unpack_ext = "ps1"
        unpack_interpreter = ["powershell", "-NoProfile", "-ExecutionPolicy", "Bypass", "-File"]
    else:
        unarchive_cmd, needs_strip_prefix = _unarchive_cmd(ext_type, archive, strip_prefix)
        unpack_ext = "sh"
        unpack_interpreter = ["/bin/sh"]

    output = ctx.actions.declare_output(output_name, dir = True, has_content_based_path = has_content_based_path)
    script_output = ctx.actions.declare_output(output_name + "_tmp", dir = True, has_content_based_path = False) if needs_strip_prefix else output

    if exec_is_windows:
        script_lines = _windows_unpack_ps1(script_output.as_output(), archive, ext_type, strip_prefix, exclude_flags)
    else:
        script_lines = [
            cmd_args(script_output.as_output(), format = "mkdir -p {}"),
            cmd_args(script_output.as_output(), format = "cd {}"),
            cmd_args([unarchive_cmd] + exclude_flags, delimiter = " ", quote = "shell", relative_to = script_output.as_output()),
        ]

    script, _ = ctx.actions.write(
        "{}_unpack.{}".format(output_name, unpack_ext),
        script_lines,
        is_executable = True,
        allow_args = True,
        has_content_based_path = False,
    )

    ctx.actions.run(
        cmd_args(
            unpack_interpreter + [script],
            hidden = exclude_hidden + [archive, script_output.as_output()],
        ),
        category = "http_archive",
        identifier = output_name,
        prefer_local = prefer_local,
    )

    if needs_strip_prefix:
        ctx.actions.copy_dir(output.as_output(), script_output.project(strip_prefix), has_content_based_path = has_content_based_path)

    if type(sub_targets) == type([]):
        sub_targets = {path: [DefaultInfo(default_output = output.project(path))] for path in sub_targets}
    elif type(sub_targets) == type({}):
        sub_targets = {name: [DefaultInfo(default_outputs = [output.project(path) for path in paths])] for name, paths in sub_targets.items()}
    else:
        fail("sub_targets must be a list or dict")

    return output, sub_targets
