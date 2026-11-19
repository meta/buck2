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

def http_file_shared(
    actions: AnalysisActions,
    name: str,
    url: str,
    vpnless_url: [None, str],
    is_executable: bool,
    is_exploded_zip: bool,
    unzip_tool: [RunInfo, None],
    sha1: [None, str],
    sha256: [None, str],
    size_bytes: [None, int],
    has_content_based_path: bool,
) -> list[Provider]:
    output = actions.declare_output(name, has_content_based_path = has_content_based_path)
    downloaded_output = actions.declare_output("exploded_zip", has_content_based_path = has_content_based_path) if is_exploded_zip else output
    actions.download_file(
        downloaded_output,
        url,
        vpnless_url = vpnless_url,
        is_executable = is_executable,
        sha1 = sha1,
        sha256 = sha256,
        size_bytes = size_bytes,
    )

    if is_exploded_zip:
        actions.run(
            cmd_args([
                unzip_tool,
                "--src",
                downloaded_output,
                "--dst",
                output.as_output(),
            ]),
            category = "exploded_zip_unzip",
            local_only = sha1 == None,
        )

    providers = [
        DefaultInfo(default_output = output),
        materialization_test([downloaded_output, output]),
    ]
    if is_executable:
        providers.append(RunInfo(args = [output]))
    return providers

def http_file_impl(ctx: AnalysisContext) -> list[Provider]:
    expect(len(ctx.attrs.urls) == 1, "multiple `urls` not supported: {}", ctx.attrs.urls)
    expect(len(ctx.attrs.vpnless_urls) < 2, "multiple `vpnless_urls` not supported: {}", ctx.attrs.vpnless_urls)
    if len(ctx.attrs.vpnless_urls) > 0:
        vpnless_url = ctx.attrs.vpnless_urls[0]
    else:
        vpnless_url = None
    return http_file_shared(
        ctx.actions,
        name = value_or(ctx.attrs.out, ctx.label.name),
        url = ctx.attrs.urls[0],
        vpnless_url = vpnless_url,
        sha1 = ctx.attrs.sha1,
        sha256 = ctx.attrs.sha256,
        is_executable = ctx.attrs.executable or False,
        is_exploded_zip = False,
        unzip_tool = None,
        size_bytes = ctx.attrs.size_bytes,
        has_content_based_path = ctx.attrs.has_content_based_path,
    )

http_file = prelude_rule(
    name = "http_file",
    impl = http_file_impl,
    docs = """
        An `http_file()` rule is used to download files from the Internet to be used as
        dependencies for other rules. This rule only downloads single files, and can
        optionally make them executable (see `http_file()executable`)
        These rules are downloaded by running `fetch`, or can
        be downloaded as part of `build` by setting `.buckconfig`
    """,
    examples = """
        Using `http_file()`, third party packages can be downloaded from
         an `https` URL and used in java libraries.

        ```
        http_file(
          name = 'guava-23-bin',
          urls = [
            'http://search.maven.org/remotecontent?filepath=com/google/guava/guava/23.0/guava-23.0.jar',
          ],
          sha256 = '7baa80df284117e5b945b19b98d367a85ea7b7801bd358ff657946c3bd1b6596',
        )
        http_file(
          name = 'guava-23-sources',
          urls = [
            'http://search.maven.org/remotecontent?filepath=com/google/guava/guava/23.0/guava-23.0-sources.jar',
          ],
          sha256 = '37fe8ba804fb3898c3c8f0cbac319cc9daa58400e5f0226a380ac94fb2c3ca14',
        )

        prebuilt_java_library(
          name = 'guava-23',
          binary_jar = ':guava-23-bin',
          source_jar = ':guava-23-source',
        )
        ```

         Tooling can also be fetched with `http_file()` and used by a `genrule()`.

        ```
        genrule(
          name="my-thrift-lib-cpp2",
          cmd="$(exe :thrift-compiler-bin) --gen cpp2 -o $OUT $(location //:thrift-file)",
          out="gen-cpp2",
        )

        http_file(
          name = 'thrift-compiler-bin',
          url = 'https://internal-mirror.example.com/bin/thrift-compiler',
          sha256 = 'c24932ccabb66fffb2d7122298f7f1f91e0b1f14e05168e3036333f84bdf58dc',
          executable = True,
        )
        ```

         Here's an example of a `http_file()` using a mvn URI which uses a Maven classifier.

        ```
        http_file(
          name = 'guava-23-bin',
          urls = [
            'mvn:com.google.guava:guava:jar:23.0',
          ],
          sha256 = '7baa80df284117e5b945b19b98d367a85ea7b7801bd358ff657946c3bd1b6596',
        )
        ```
    """,
    further = None,
    attrs = (
        # @unsorted-dict-items
        remote_common.urls_arg()
        | remote_common.sha256_arg()
        | {
            "executable": attrs.option(
                attrs.bool(),
                default = None,
                doc = """
                Whether or not the file should be made executable after downloading. If true,
                 this can also be used via `run` and the
                 `$(exe )` `string parameter macros`
            """,
            ),
            "out": attrs.option(
                attrs.string(),
                default = None,
                doc = """
                An optional name to call the downloaded artifact. Buck will generate a default name if one is not
                 provided that uses the `name` of the rule.
            """,
            ),
            "sha1": attrs.option(attrs.string(), default = None),
            "size_bytes": attrs.option(attrs.int(), default = None),
        }
        | buck.has_content_based_path_attr()
        | buck.licenses_arg()
        | buck.labels_arg()
        | buck.contacts_arg()
    ),
)
