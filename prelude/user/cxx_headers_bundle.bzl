# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

load("@prelude//:artifacts.bzl", "ArtifactGroupInfo")
load("@prelude//:paths.bzl", "paths")
load("@prelude//cxx:preprocessor.bzl", "CPreprocessorInfo", "cxx_merge_cpreprocessors")
load("@prelude//decls:common.bzl", "buck")
load("@prelude//utils:expect.bzl", "expect")
load(":rule_spec.bzl", "RuleRegistrationSpec")

def _headers(ctx: AnalysisContext, deps: list[Dependency]) -> dict[str, Artifact]:
    headers = {}

    pp_info = cxx_merge_cpreprocessors(ctx.actions, [], [d[CPreprocessorInfo] for d in deps])
    for pps in pp_info.set.traverse():
        for pp in pps:
            for hdr in pp.headers:
                headers[paths.join(hdr.namespace, hdr.name)] = hdr.artifact

    return headers

def _impl(ctx: AnalysisContext) -> list[Provider]:
    headers = _headers(ctx, ctx.attrs.deps)
    if ctx.attrs.limit != None:
        expect(
            len(headers) <= ctx.attrs.limit,
            "Expected at most {} headers, but transitively pulled in {}",
            ctx.attrs.limit,
            len(headers),
        )
    output = ctx.actions.symlinked_dir(ctx.label.name, headers, has_content_based_path = False)
    artifacts = [output.project(name, hide_prefix = True) for name in headers]
    return [
        ArtifactGroupInfo(artifacts = artifacts),
        DefaultInfo(default_outputs = [output]),
    ]

registration_spec = RuleRegistrationSpec(
    name = "cxx_headers_bundle",
    doc = """
        Bundles transitive exported C/C++ headers from C/C++ libraries, allowing
        them to e.g. be consumed via the `resources`s parameter in other rules.
        The headers maintain their `#include` paths, as defined by the C/C++
        libraries that export them.
    """,
    impl = _impl,
    attrs = buck.labels_arg()
    | buck.contacts_arg()
    | {
        "deps": attrs.list(
            attrs.dep(providers = [CPreprocessorInfo]),
            default = [],
            doc = """
                Bundle the exported C/C++ headers from these (transitive) deps.
            """,
        ),
        "limit": attrs.option(
            attrs.int(),
            default = None,
            doc = """
                Enforce that we don't bundle more than this number of headers.
            """,
        ),
    },
)
