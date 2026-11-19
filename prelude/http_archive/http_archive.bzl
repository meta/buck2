# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

load("@prelude//:prelude_rule.bzl", "prelude_rule")
load("@prelude//decls:common.bzl", "buck")
load("@prelude//decls:remote_common.bzl", "remote_common")
load("@prelude//utils:expect.bzl", "expect")
load("@prelude//utils:materialization_test.bzl", "materialization_test")
load("@prelude//utils:utils.bzl", "value_or")
load(":exec_deps.bzl", "HttpArchiveExecDeps")
load(":unarchive.bzl", "archive_type", "unarchive")

def http_archive_impl(ctx: AnalysisContext) -> list[Provider]:
    expect(len(ctx.attrs.urls) == 1, "multiple `urls` not supported: {}".format(ctx.attrs.urls))
    expect(len(ctx.attrs.vpnless_urls) < 2, "multiple `vpnless_urls` not supported: {}".format(ctx.attrs.vpnless_urls))

    # The HTTP download is local so it makes little sense to run actions
    # remotely, unless we can defer them. We'll be able to defer if we provide
    # a digest that the daemon's digest config natively supports.
    digest_config = ctx.actions.digest_config()
    prefer_local = True
    if ctx.attrs.sha1 != None and digest_config.allows_sha1():
        prefer_local = False
    elif ctx.attrs.sha256 != None and digest_config.allows_sha256():
        prefer_local = False

    ext_type = archive_type(ctx.attrs.urls[0], ctx.attrs.type)

    # Download archive.
    archive = ctx.actions.declare_output("archive." + ext_type, has_content_based_path = ctx.attrs.has_content_based_path)
    url = ctx.attrs.urls[0]
    vpnless_url = None if len(ctx.attrs.vpnless_urls) == 0 else ctx.attrs.vpnless_urls[0]
    ctx.actions.download_file(
        archive.as_output(),
        url,
        vpnless_url = vpnless_url,
        sha1 = ctx.attrs.sha1,
        sha256 = ctx.attrs.sha256,
        size_bytes = ctx.attrs.size_bytes,
        has_content_based_path = ctx.attrs.has_content_based_path,
    )

    output, sub_targets = unarchive(
        ctx,
        archive = archive,
        output_name = value_or(ctx.attrs.out, ctx.label.name),
        ext_type = ext_type,
        excludes = ctx.attrs.excludes,
        strip_prefix = ctx.attrs.strip_prefix,
        sub_targets = ctx.attrs.sub_targets,
        exec_deps = ctx.attrs.exec_deps[HttpArchiveExecDeps],
        prefer_local = prefer_local,
        has_content_based_path = ctx.attrs.has_content_based_path,
    )

    return [
        DefaultInfo(
            default_output = output,
            sub_targets = sub_targets,
        ),
        materialization_test([archive, output]),
    ]

http_archive = prelude_rule(
    name = "http_archive",
    impl = http_archive_impl,
    docs = """
        An `http_archive()` rule is used to download and extract archives
        from the Internet to be used as dependencies for other rules. These rules are
        downloaded by running `fetch`, or can be downloaded as part of
        `build` by setting `.buckconfig`
    """,
    examples = """
        Using `http_archive()`, third party packages can be downloaded from
         an `https` URL and used in other library types.

        ```
        http_archive(
          name = 'thrift-archive',
          urls = [
            'https://internal-mirror.example.com/bin/thrift-compiler-0.1.tar.gz.badextension',
          ],
          sha256 = '7baa80df284117e5b945b19b98d367a85ea7b7801bd358ff657946c3bd1b6596',
          type='tar.gz',
          strip_prefix='thrift-compiler-0.1'
        )

        genrule(
          name = 'thrift-compiler-bin',
          out = 'thrift',
          cmd = 'cp $(location :thrift-archive)/bin/thrift $OUT',
          executable = True,
        )

        genrule(
          name="my-thrift-lib-cpp2",
          cmd="$(exe :thrift-compiler-bin) --gen cpp2 -o $OUT $(location //:thrift-file)",
          out="gen-cpp2",
        )
        ```
    """,
    further = None,
    attrs = (
        # @unsorted-dict-items
        remote_common.urls_arg()
        | remote_common.sha256_arg()
        | remote_common.unarchive_args()
        | {
            "sha1": attrs.option(attrs.string(), default = None),
            "size_bytes": attrs.option(attrs.int(), default = None),
        }
        | buck.has_content_based_path_attr()
        | buck.licenses_arg()
        | buck.labels_arg()
        | buck.contacts_arg()
    ),
)
