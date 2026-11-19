# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

GoCompilerInfo = provider(
    doc = "Information about how to invoke the go compiler.",
    fields = ["compiler_path", "GOROOT"],
)

def _go_toolchain_impl(ctx):
    download = _download_toolchain(ctx)

    compiler_dst = ctx.actions.declare_output("compiler.exe" if host_info().os.is_windows else "compiler", has_content_based_path = False)

    cmd = cmd_args()
    if host_info().os.is_windows:
        compiler_src = cmd_args(download, format = "{}\\go\\bin\\go.exe", relative_to = (compiler_dst, 1))
        cmd.add([ctx.attrs._symlink_bat, compiler_dst.as_output(), compiler_src])
    else:
        compiler_src = cmd_args(download, format = "{}/go/bin/go", relative_to = (compiler_dst, 1))
        cmd.add(["ln", "-sf", compiler_src, compiler_dst.as_output()])

    ctx.actions.run(cmd, category = "cp_compiler")
    return [DefaultInfo(default_output = download), GoCompilerInfo(compiler_path = compiler_dst, GOROOT = "")]

go_toolchain = rule(
    impl = _go_toolchain_impl,
    attrs = {
        "archive_extension": attrs.string(),
        "platform": attrs.string(),
        "sha256": attrs.string(),
        "version": attrs.string(),
        "_symlink_bat": attrs.default_only(attrs.source(default = "toolchains//:symlink.bat")),
    },
)

def _download_toolchain(ctx: AnalysisContext):
    archive_extension = ctx.attrs.archive_extension
    url = "https://go.dev/dl/go" + ctx.attrs.version + "." + ctx.attrs.platform + "." + archive_extension
    sha256 = ctx.attrs.sha256

    # Download archive.
    archive = ctx.actions.declare_output("archive." + archive_extension, has_content_based_path = False)
    ctx.actions.download_file(archive.as_output(), url, sha256 = sha256)

    output = ctx.actions.declare_output(ctx.label.name, has_content_based_path = False)

    # Unpack archive to output directory.
    compress_flag = "-z"
    script_name = "unpack.bat" if host_info().os.is_windows else "unpack.sh"

    script_content = []
    if host_info().os.is_windows:
        script_content.append(cmd_args(output, format = "mkdir {}"))
    else:
        script_content.append(cmd_args(output, format = "mkdir -p {}"))
    if host_info().os.is_windows:
        script_content.extend([
            cmd_args(output, format = "cd {}"),
            cmd_args(["unzip", archive], delimiter = " ", relative_to = output),
        ])
    else:
        script_content.extend([
            cmd_args(output, format = "cd {}"),
            cmd_args(["tar", compress_flag, "-x", "-f", archive], delimiter = " ", relative_to = output),
        ])
    script, _ = ctx.actions.write(
        script_name,
        script_content,
        is_executable = True,
        allow_args = True,
        has_content_based_path = False,
    )

    cmd = cmd_args(
        ([] if host_info().os.is_windows else ["/bin/sh"]) + [script],
        hidden = [archive, output.as_output()],
    )

    ctx.actions.run(cmd, category = "extract_go_toolchain")

    return output

def _toolchain_config():
    version = "1.20.7"
    os = host_info().os
    arch = host_info().arch
    if os.is_windows:
        return struct(
            sha256 = "736dc6c7fcab1c96b682c8c93e38d7e371e62a17d34cb2c37d451a1147f66af9",
            platform = "windows-amd64",
            archive_extension = "zip",
            version = version,
        )
    if os.is_macos:
        if arch.is_aarch64:
            return struct(
                sha256 = "eea1e7e4c2f75c72629050e6a6c7c46c446d64056732a7787fb3ba16ace1982e",
                platform = "darwin-arm64",
                archive_extension = "tar.gz",
                version = version,
            )
        elif arch.is_x86_64:
            return struct(
                sha256 = "785170eab380a8985d53896808b0a71336d0ea60e0a26099b4ccec77798b1cf4",
                platform = "darwin-amd64",
                archive_extension = "tar.gz",
                version = version,
            )
        else:
            fail("unrecognized architecture: couldn't select macOS go toolchain")

    # Default linux
    return struct(
        sha256 = "f0a87f1bcae91c4b69f8dc2bc6d7e6bfcd7524fceec130af525058c0c17b1b44",
        platform = "linux-amd64",
        archive_extension = "tar.gz",
        version = version,
    )

toolchain_config = _toolchain_config()
