# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

load("@prelude//:http_file.bzl", "http_file_shared")
load("@prelude//:prelude_rule.bzl", "prelude_rule")
load("@prelude//decls:common.bzl", "buck", "validate_uri")
load("@prelude//decls:remote_common.bzl", "remote_common")
load("@prelude//utils:expect.bzl", "expect")
load("@prelude//utils:utils.bzl", "value_or")

_DEFAULT_MAVEN_REPO = read_config("http", "maven_repo", "https://repo1.maven.org/maven2")
_MAVEN_REPO_OVERRIDE = read_config("http", "maven_repo_override")

def _from_mvn_url(url: str) -> str:
    """
    Convert `mvn:` style URIs to a URL.
    """

    count = url.count(":")
    mod = ""

    if count == 4:
        mvn, group, id, typ, version = url.split(":")
        repo = _DEFAULT_MAVEN_REPO
    elif count == 5:
        mvn, group, id, typ, mod, version = url.split(":")
        mod = "-" + mod
        repo = _DEFAULT_MAVEN_REPO
    elif count == 6:
        mvn, repo_protocol, repo_host, group, id, typ, version = url.split(":")
        repo = repo_protocol + ":" + repo_host
    elif count == 7:
        mvn, repo_protocol, repo_host, group, id, typ, mod, version = url.split(":")
        mod = "-" + mod
        repo = repo_protocol + ":" + repo_host
    else:
        fail("Unsupported mvn URL scheme: " + url + " (" + str(count) + ")")

    expect(mvn == "mvn")

    group = group.replace(".", "/")

    if typ == "src":
        ext = "-sources.jar"
    else:
        ext = "." + typ

    if _MAVEN_REPO_OVERRIDE:
        repo = _MAVEN_REPO_OVERRIDE

    return "{repo}/{group}/{id}/{version}/{id}-{version}{mod}{ext}".format(
        repo = repo,
        group = group,
        id = id,
        version = version,
        ext = ext,
        mod = mod,
    )

# Implementation of the `remote_file` build rule.
def remote_file_impl(ctx: AnalysisContext) -> list[Provider]:
    url = ctx.attrs.url
    if url.startswith("mvn:"):
        url = _from_mvn_url(url)
    return http_file_shared(
        ctx.actions,
        name = value_or(ctx.attrs.out, ctx.label.name),
        url = url,
        vpnless_url = ctx.attrs.vpnless_url,
        is_executable = ctx.attrs.type == "executable",
        is_exploded_zip = ctx.attrs.type == "exploded_zip",
        unzip_tool = ctx.attrs._unzip_tool[RunInfo],
        sha1 = ctx.attrs.sha1,
        sha256 = ctx.attrs.sha256,
        size_bytes = ctx.attrs.size_bytes,
        has_content_based_path = ctx.attrs.has_content_based_path,
    )

RemoteFileType = ["data", "executable", "exploded_zip"]

remote_file = prelude_rule(
    name = "remote_file",
    impl = remote_file_impl,
    docs = """
        A `remote_file()` rule is used to download files from the Internet to be used as
        dependencies for other rules. These rules are downloaded by running `fetch`, or can
        be downloaded as part of `build`. See the note there about the
         `.buckconfig` setting to configure that.
    """,
    examples = """
        Here's an example of a `remote_file()` using an `https` URL.

        ```
        remote_file(
          name = 'android-ndk-r10e-darwin-x86_64',
          url = 'https://dl.google.com/android/ndk/android-ndk-r10e-darwin-x86_64.bin',
          sha1 = 'b57c2b9213251180dcab794352bfc9a241bf2557',
        )
        ```

         Here's an example of a `remote_file()` using a `mvn` URL being referenced
         by a `prebuilt_jar()`.

        ```
        prebuilt_jar(
          name = 'jetty-all',
          binary_jar = 'jetty-all-9.2.10.v20150310.jar',
          source_jar = ':jetty-source',
        )

        remote_file(
          name = 'jetty-source',
          out = 'jetty-all-9.2.10.v20150310-sources.jar',
          url = 'mvn:org.eclipse.jetty.aggregate:jetty-all:src:9.2.10.v20150310',
          sha1 = '311da310416d2feb3de227081d7c3f48742d7075',
        )
        ```

         Here's an example of a `remote_file()` using a `mvn` URI which uses a
         non-default maven repository host.

        ```
        remote_file(
          name = 'jetty-source',
          out = 'jetty-all-9.2.10.v20150310-sources.jar',
          url = 'mvn:https://maven-repo.com:org.eclipse.jetty.aggregate:jetty-all:src:9.2.10.v20150310',
          sha1 = '311da310416d2feb3de227081d7c3f48742d7075',
        )
        ```

         Here's an example of a `remote_file()` using a `mvn` URI which uses a
         Maven classifier.

        ```
        remote_file(
          name = 'groovy-groovysh-indy',
          out = 'jetty-all-9.2.10.v20150310-sources.jar',
          url = 'mvn:org.codehaus.groovy:groovy-groovysh:jar:indy:2.4.1',
          sha1 = '1600fde728c885cc9506cb102deb1b494bd7c130',
        )
        ```
    """,
    further = None,
    attrs = (
        # @unsorted-dict-items
        {
            "url": attrs.string(
                validate = validate_uri,
                doc = """
                You can specify an `http`, `https`, or a `mvn` URL. If you
                 specify a `mvn` URL, it will be decoded as described in the
                 javadocs for MavenUrlDecoder See the example section below.
            """,
            ),
            "vpnless_url": attrs.option(
                attrs.string(),
                default = None,
                doc = """
                An optional additional URL from which this resource can be downloaded when
                  off VPN. Meta-internal only.
            """,
            ),
            "size_bytes": attrs.option(
                attrs.int(),
                default = None,
                doc = """
                Size in bytes of the downloaded artifact. Buck verifies this is correct and fails the fetch
                 command if it doesn't match (only when a SHA-1 hash is used).
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
            "type": attrs.option(
                attrs.enum(RemoteFileType),
                default = None,
                doc = """
                An optional type of the downloaded file.

                `data`
                Regular data file.
                `executable`

                 Executable file. Buck will ensure that output has appropriate permissions if applicable.

                `exploded_zip`

                 Zip archive which will be automatically unzipped into an output directory.
            """,
            ),
        }
        | buck.has_content_based_path_attr()
        | buck.licenses_arg()
        | buck.labels_arg()
        | buck.contacts_arg()
        | {
            "sha1": attrs.option(
                attrs.string(),
                default = None,
                doc = """
                The [`SHA-1`](//wikipedia.org/wiki/SHA-1) hash of the downloaded artifact.
                 Buck verifies this is correct and fails the fetch command if it doesn't match in order to
                 guarantee repeatable builds.
            """,
            ),
            "_unzip_tool": attrs.default_only(attrs.exec_dep(providers = [RunInfo], default = "prelude//zip_file/tools:unzip")),
        }
        | remote_common.sha256_arg()
    ),
)
